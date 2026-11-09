//! Exercises the catalogue commands against a large catalogue on disk.
//!
//! Unit tests run against a fresh in-memory schema, which proves the SQL is
//! valid but not that it answers usefully over a large catalogue. This runs
//! the same functions the frontend invokes, against a database file, and
//! fails loudly on an empty or nonsensical answer.
//!
//! By default the catalogue is a fixture built here, in a scratch directory:
//! 600 categories, 12,000 channels and an hour of guide either side of now.
//! With `ZAPPER_ALLOW_REAL_ACCOUNT=1` it is the mirror a configured copy of
//! zapper has synced (`~/.local/share/zapper`) instead — which this run also
//! writes settings to, and puts back.
//!
//! No window, no network, no account either way: reads come from the mirror.

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

fn main() {
    // Before the runtime exists: this may change the environment.
    let preset = std::env::var_os(zapper_lib::paths::HOME_OVERRIDE).is_some();
    let real = zapper_lib::harness::isolate("ipc_check");
    if !real {
        if let Err(e) = seed_fixture() {
            eprintln!("FAIL: building the fixture catalogue: {e}");
            std::process::exit(1);
        }
    }
    let verdict = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(run());
    // The scratch directory this run made for itself; one a caller chose is
    // the caller's to clean up.
    if !real && !preset {
        if let Some(root) = zapper_lib::paths::data_dir().parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
    let what = if real { "the real mirror" } else { "a fixture catalogue" };
    match verdict {
        Ok(()) => println!("PASS: catalogue commands answer from {what}"),
        Err(e) => {
            eprintln!("FAIL: {e}");
            std::process::exit(1);
        }
    }
}

/// Fills the scratch database with a catalogue the size of a real one.
///
/// Shaped like an Xtream server's: numbered categories, channels with a
/// guide id, some with an archive, a separator here and there, and enough
/// of them under one name that search has something to rank.
fn seed_fixture() -> anyhow::Result<()> {
    const CATEGORIES: i64 = 600;
    const CHANNELS: i64 = 12_000;
    const NAMES: [&str; 6] = ["Northwind One", "Harbour Sports", "Meridian News", "Lakeside", "Cinema", "Kids"];
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    zapper_lib::catalog::db::with_db(|c| {
        let tx = c.unchecked_transaction()?;
        for cat in 0..CATEGORIES {
            tx.execute(
                "INSERT INTO categories (id, name, sort) VALUES (?1, ?2, ?3)",
                rusqlite::params![cat.to_string(), format!("Category {cat}"), cat],
            )?;
        }
        for n in 0..CHANNELS {
            let id = 100_000 + n;
            // Uneven on purpose, so some categories are large.
            let cat = if n % 3 == 0 { 0 } else { n % CATEGORIES };
            let separator = n % 997 == 0;
            let name = if separator {
                format!("##### Section {n} #####")
            } else {
                format!("{} {n}", NAMES[(n % NAMES.len() as i64) as usize])
            };
            let archive = !separator && n % 7 == 0;
            tx.execute(
                "INSERT INTO channels (stream_id, category_id, name, clean_name, num,
                                       epg_channel_id, has_archive, archive_days, is_separator)
                 VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    id,
                    cat.to_string(),
                    name,
                    n + 1,
                    format!("epg-{id}"),
                    archive,
                    archive.then_some(3),
                    separator,
                ],
            )?;
            if !separator && n % 4 == 0 {
                for (k, title) in ["Earlier", "Now", "Later"].into_iter().enumerate() {
                    let start = now - 3600 + k as i64 * 3600 - 1800;
                    tx.execute(
                        "INSERT INTO epg (channel_ref, start, stop, title) VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![format!("epg-{id}"), start, start + 3600, title],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    })?;
    zapper_lib::catalog::status::mark_synced(now);
    Ok(())
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

    let hits = cmd::search("northwind one".into(), 20).await?;
    println!("search 'northwind one': {} hits", hits.len());
    check!("fuzzy search finds something", !hits.is_empty());
    check!(
        "search never returns separators",
        hits.iter().all(|c| !c.is_separator)
    );

    // Typo tolerance is the whole reason for nucleo over FTS5.
    let fuzzy = cmd::search("northwindone".into(), 20).await?;
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

    // --- settings ---------------------------------------------------------

    let before = cmd::settings().await?;
    println!(
        "settings: rewind={}MB hwdec={} tonemap={} guide={}h backdrop={:?}",
        before.rewind_mb,
        before.hardware_decode,
        before.tone_map_hdr,
        before.guide_hours,
        before.subtitle.backdrop
    );
    check!("HDR is tone-mapped by default", before.tone_map_hdr);
    check!(
        "subtitles default to a backdrop at 0.25",
        (before.subtitle.backdrop_opacity - 0.25).abs() < 1e-9
    );

    let before_mb = before.rewind_mb;
    let mut changed = before.clone();
    changed.rewind_mb = 900;
    changed.subtitle.size = 44;
    cmd::set_settings(changed).await?;
    let after = cmd::settings().await?;
    check!("a saved setting reads back", after.rewind_mb == 900);
    check!("nested subtitle fields persist", after.subtitle.size == 44);
    cmd::set_settings(before).await?;
    let restored = cmd::settings().await?;
    check!("settings restore cleanly", restored.rewind_mb == before_mb);

    // --- guide rows -------------------------------------------------------

    let rows = cmd::guide_rows(ids.clone(), now - 3600, now + 3600).await?;
    println!("guide_rows for {} channels: {}", ids.len(), rows.len());
    check!("a row per channel, none dropped", rows.len() == ids.len());
    check!(
        "rows come back in the order asked for",
        rows.iter().zip(&ids).all(|(r, id)| r.stream_id == *id)
    );
    check!(
        "every row carries its channel name",
        rows.iter().all(|r| !r.name.is_empty())
    );
    let with_prog = rows.iter().filter(|r| !r.programmes.is_empty()).count();
    println!("  ...of which {with_prog} have programmes");
    check!(
        "programmes fall inside the window",
        rows.iter()
            .flat_map(|r| &r.programmes)
            .all(|p| p.start < now + 3600 && p.stop > now - 3600)
    );

    Ok(())
}
