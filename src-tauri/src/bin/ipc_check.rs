//! Exercises the catalogue commands against the real, synced database.
//!
//! Unit tests run against a fresh in-memory schema, which proves the SQL is
//! valid but not that it answers usefully over a real, large catalogue. This
//! runs the same functions the frontend invokes, against
//! `~/.local/share/zapper`, and fails loudly on an empty or nonsensical
//! answer.
//!
//! No window, no network, no account: reads come from the mirror.

use zapper_lib::ipc::catalog_cmds as cmd;

macro_rules! check {
    ($label:expr, $cond:expr) => {
        if $cond {
            println!("  ok    {}", $label);
        } else {
            println!("  FAIL  {}", $label);
            return Err($label.to_string());
        }
    };
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    match run().await {
        Ok(()) => println!("PASS: catalogue commands answer from the local mirror"),
        Err(e) => {
            eprintln!("FAIL: {e}");
            std::process::exit(1);
        }
    }
}

async fn run() -> Result<(), String> {
    let cats = cmd::categories().await?;
    println!("categories: {}", cats.len());
    check!("more than 500 categories", cats.len() > 500);
    check!(
        "every category carries a count",
        cats.iter().all(|c| c.channel_count >= 0)
    );
    check!(
        "some category is large",
        cats.iter().any(|c| c.channel_count > 100)
    );

    let page = cmd::channels(None, 0, 50).await?;
    println!("first page: {} rows", page.len());
    check!("a full page came back", page.len() == 50);
    check!(
        "ids are 64-bit clean",
        page.iter().all(|c| c.stream_id > 0)
    );
    check!(
        "names are populated",
        page.iter().all(|c| !c.name.is_empty())
    );

    // Paging must not repeat rows: an off-by-one here would silently show the
    // same channels forever.
    let page2 = cmd::channels(None, 50, 50).await?;
    let overlap = page
        .iter()
        .filter(|a| page2.iter().any(|b| b.stream_id == a.stream_id))
        .count();
    check!("the second page is disjoint", overlap == 0);

    let hits = cmd::search("nwn one".into(), 20).await?;
    println!("search 'nwn one': {} hits", hits.len());
    check!("fuzzy search finds something", !hits.is_empty());
    check!(
        "search never returns separators",
        hits.iter().all(|c| !c.is_separator)
    );

    // Typo tolerance is the whole reason for nucleo over FTS5.
    let fuzzy = cmd::search("nwnone".into(), 20).await?;
    check!("subsequence matching works", !fuzzy.is_empty());

    let archived: Vec<_> = page.iter().filter(|c| c.has_archive).collect();
    println!("archive-capable in first page: {}", archived.len());
    check!(
        "archive days accompany the archive flag",
        archived.iter().all(|c| c.archive_days.is_some())
    );

    let ids: Vec<i64> = hits.iter().take(5).map(|c| c.stream_id).collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let g = cmd::guide(ids.clone(), now - 3600, now + 3600).await?;
    println!("guide rows for {} channels: {}", ids.len(), g.len());
    check!("a guide row per channel requested", g.len() == ids.len());

    let st = cmd::sync_status().await?;
    println!(
        "sync status: phase={:?} channels={} last_synced={:?}",
        st.phase, st.channels_total, st.last_synced
    );
    check!("the mirror reports its size", st.channels_total > 10_000);
    check!("a previous sync is remembered", st.last_synced.is_some());

    Ok(())
}
