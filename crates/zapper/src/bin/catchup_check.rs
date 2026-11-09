//! Verifies catch-up end to end against the fixture server: that a
//! request for "five minutes ago" asks the server for five minutes ago, in
//! the server's time zone rather than this computer's; that Live leaves the
//! catch-up programme for the channel, live; that a request at the far edge
//! of the archive is moved inside it; and that one older than the archive
//! says so.
//!
//! Usage, with the server and this computer in different zones:
//!
//! ```sh
//! tools/fake_stream.py --port 8788 --panel-utc-offset 1 --archive-days 3 &
//! TZ=America/New_York cargo run --features harness --bin catchup_check
//! ```
//!
//! `ZAPPER_PANEL_BASE` moves the server (default `http://127.0.0.1:8788`);
//! it must be on this machine. The run is on a scratch account whose
//! server is that one: never the account on this machine, and never a
//! provider. Audio is off (`ao=null`) before anything is loaded. The player
//! is mpv in a window of its own (`surface::host`): run it inside a desktop
//! session, the headless gnome-shell for anything unattended.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("catchup_check only runs on Linux");
    std::process::exit(2);
}

/// Exits without unwinding, as `player_check` does: mpv's threads, its
/// window among them, are still running.
#[cfg(target_os = "linux")]
fn finish(code: i32) -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(code)
}

#[cfg(target_os = "linux")]
fn main() {
    tracing_subscriber::fmt::init();
    // Never the real account: this run saves one of its own.
    if zapper_lib::harness::real_account_allowed() {
        eprintln!("FAIL: catchup_check saves an account, so it never runs on the real one");
        std::process::exit(2);
    }
    zapper_lib::harness::isolate("catchup_check");
    // The channel is played through the account, not as a bare fixture.
    std::env::remove_var("ZAPPER_FIXTURE_BASE");
    std::env::remove_var("ZAPPER_FIXTURE_NAME");
    let base = std::env::var("ZAPPER_PANEL_BASE").unwrap_or_else(|_| zapper_lib::harness::FIXTURE_BASE.to_owned());
    if let Err(e) = zapper_lib::harness::local_url(&base) {
        eprintln!("FAIL: ZAPPER_PANEL_BASE: {e}");
        std::process::exit(2);
    }

    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(180));
        eprintln!("FAIL: the run did not finish within 180s");
        finish(1);
    });
    // mpv in a window of its own, installed as the player, with no audio
    // output at all (`ao=null`).
    let opened = zapper_lib::surface::host::open_host(zapper_lib::surface::host::HostOptions {
        title: "zapper catchup_check".to_owned(),
        muted: true,
    });
    if let Err(e) = opened {
        eprintln!("FAIL: opening the player: {e}");
        finish(1);
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| {
            eprintln!("FAIL: starting the runtime: {e}");
            finish(1)
        });
    // The player's heartbeat, as the app starts it.
    zapper_lib::player::start(rt.handle());
    let verdict = rt.block_on(async {
        let verdict = run(&base).await;
        let _ = zapper_lib::player::api::stop().await;
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        verdict
    });
    match verdict {
        Ok(msg) => {
            println!("PASS: {msg}");
            finish(0);
        }
        Err(e) => {
            eprintln!("FAIL: {e}");
            finish(1);
        }
    }
}

/// The channel played, and how deep its archive is said to be.
#[cfg(target_os = "linux")]
const STREAM: i64 = 1;
#[cfg(target_os = "linux")]
const ARCHIVE_DAYS: i64 = 3;

/// One request the server saw, from its `/requests`.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, serde::Deserialize)]
struct Seen {
    kind: String,
    stream: String,
    #[serde(default)]
    stamp: Option<String>,
    #[serde(default)]
    start: Option<i64>,
    #[serde(default)]
    status: Option<u16>,
    /// Other streams the server was still sending when this was asked for.
    #[serde(default)]
    open_streams: Option<u32>,
}

#[cfg(target_os = "linux")]
async fn seen(base: &str) -> Result<Vec<Seen>, String> {
    reqwest::get(format!("{}/requests", base.trim_end_matches('/')))
        .await
        .map_err(|e| format!("asking the server what it saw: {e}"))?
        .json()
        .await
        .map_err(|e| format!("reading what the server saw: {e}"))
}

#[cfg(target_os = "linux")]
async fn run(base: &str) -> Result<String, String> {
    use std::time::Duration;
    use zapper_lib::catalog::{db, origin, panel_clock};
    use zapper_lib::player::api as cmds;
    use zapper_lib::ipc::types::PlaybackState;
    use zapper_lib::xtream::{Credentials, XtreamClient};

    // --- a scratch account on the fixture server, synced for its clock ----
    let creds = Credentials {
        host: base.to_owned(),
        username: "check".into(),
        password: "check".into(),
    };
    zapper_lib::catalog::credentials::save(&creds).map_err(|e| format!("saving the account: {e:#}"))?;
    zapper_lib::player::controller::forget_source();
    let from = origin::of_xtream(&creds);
    let client = XtreamClient::new(creds.clone()).map_err(|e| e.to_string())?;
    // What a sync pass does before its phases.
    panel_clock::learn(&client, &from).await;
    let panel = db::with_db(|c| panel_clock::recorded(c, &from))
        .map_err(|e| e.to_string())?
        .ok_or("the server's clock was not learnt")?;
    let now = zapper_lib::catalog::activity::now_secs();
    let local = zapper_lib::localtime::utc_offset(now);
    println!(
        "[clock] server {} ; this computer {} (TZ={})",
        panel_clock::describe(panel),
        panel_clock::describe(local),
        std::env::var("TZ").unwrap_or_default()
    );
    // The channel, with its archive, as a sync would have left it.
    db::with_write_db(|c| {
        c.execute(
            "INSERT OR REPLACE INTO channels (stream_id, category_id, name, clean_name, has_archive, archive_days)
             VALUES (?1, 'c', 'Check One', 'Check One', 1, ?2)",
            rusqlite::params![STREAM, ARCHIVE_DAYS],
        )?;
        Ok(())
    })
    .map_err(|e| format!("adding the channel: {e:#}"))?;

    // --- five minutes ago -------------------------------------------------
    // As the scrubber asks: the moment, rounded down to the minute, running
    // to live.
    let now = zapper_lib::catalog::activity::now_secs();
    let five_ago = (now - 300) / 60 * 60;
    cmds::play_catchup(STREAM, five_ago, 6).await?;
    let opened = await_state(Duration::from_secs(25), |s| s.state != PlaybackState::Opening).await;
    if opened.state != PlaybackState::Playing {
        return Err(format!("the catch-up ended in {:?}: {:?} {:?}", opened.state, opened.ending, opened.error));
    }
    let asked = seen(base)
        .await?
        .into_iter()
        .rev()
        .find(|r| r.kind == "timeshift")
        .ok_or("the server saw no catch-up request")?;
    println!("[five-ago] asked {:?}, which the server reads as {:?}", asked.stamp, asked.start);
    if asked.start != Some(five_ago) {
        return Err(format!(
            "asked for {:?} ({:?}), which the server reads as {}s from five minutes ago",
            asked.stamp,
            asked.start,
            asked.start.map_or(i64::MAX, |s| s - five_ago)
        ));
    }
    tokio::time::sleep(Duration::from_secs(4)).await;
    let playing = cmds::player_state().await?;
    println!(
        "[five-ago] state={:?} at_live_edge={} behind_live={:.1}",
        playing.state, playing.at_live_edge, playing.behind_live
    );
    if playing.at_live_edge {
        return Err("a catch-up programme reads as live, so Live is not offered".into());
    }
    if zapper_lib::player::controller::now_playing().is_some_and(|p| p.live) {
        return Err("the catch-up programme is taken for the live channel".into());
    }

    // --- Live -------------------------------------------------------------
    let before = seen(base).await?.len();
    cmds::seek_live().await?;
    let live = await_state(Duration::from_secs(25), |s| {
        s.state == PlaybackState::Playing && s.at_live_edge
    })
    .await;
    let after = seen(base).await?;
    let reopened = after[before..].iter().find(|r| r.kind == "live");
    println!(
        "[live] state={:?} at_live_edge={} behind_live={:.1} ; server saw {:?}",
        live.state,
        live.at_live_edge,
        live.behind_live,
        &after[before..]
    );
    let Some(reopened) = reopened.filter(|r| r.stream == STREAM.to_string()) else {
        return Err("Live did not open the channel again".into());
    };
    // One connection at a time: the programme's was closed first.
    if reopened.open_streams != Some(0) {
        return Err(format!(
            "the channel was asked for with {:?} other stream(s) still open",
            reopened.open_streams
        ));
    }
    if after[before..].iter().any(|r| r.kind == "timeshift") {
        return Err("Live asked for the catch-up programme again".into());
    }
    if live.state != PlaybackState::Playing || !live.at_live_edge {
        return Err(format!("after Live: {:?}, at the live edge: {}", live.state, live.at_live_edge));
    }
    if !zapper_lib::player::controller::now_playing().is_some_and(|p| p.live && p.stream_id == STREAM) {
        return Err("after Live, the player does not hold the channel live".into());
    }

    // --- the archive's far edge -------------------------------------------
    let now = zapper_lib::catalog::activity::now_secs();
    let edge = (now - ARCHIVE_DAYS * 86_400) / 60 * 60;
    cmds::play_catchup(STREAM, edge, 60).await?;
    let at_edge = await_state(Duration::from_secs(25), |s| s.state != PlaybackState::Opening).await;
    let asked = seen(base)
        .await?
        .into_iter()
        .rev()
        .find(|r| r.kind == "timeshift")
        .ok_or("the server saw no request for the edge")?;
    println!("[edge] asked {:?} ({:?}s in) -> {:?}", asked.stamp, asked.start.map(|s| s - (now - ARCHIVE_DAYS * 86_400)), at_edge.state);
    if at_edge.state != PlaybackState::Playing || asked.status != Some(200) {
        return Err(format!("the archive's edge did not play: {:?} {:?}", at_edge.state, at_edge.ending));
    }
    if asked.start.is_none_or(|s| s < now - ARCHIVE_DAYS * 86_400 + 600) {
        return Err(format!("the edge was asked for without the margin: {:?}", asked.start));
    }

    // --- older than the archive --------------------------------------------
    // A channel whose archive is not known to be that short: the server is
    // asked, and refuses.
    db::with_write_db(|c| {
        c.execute("UPDATE channels SET archive_days = NULL WHERE stream_id = ?1", [STREAM])?;
        Ok(())
    })
    .map_err(|e| format!("{e:#}"))?;
    cmds::play_catchup(STREAM, edge - 86_400, 60).await?;
    let refused = await_state(Duration::from_secs(25), |s| s.state == PlaybackState::Failed).await;
    println!("[too-old] state={:?} ending={:?} at_live_edge={}", refused.state, refused.ending, refused.at_live_edge);
    let kind = refused.ending.map(|e| e.kind);
    if kind != Some(zapper_lib::ipc::ending::EndingKind::BeyondArchive) {
        return Err(format!("a request older than the archive ended as {kind:?}"));
    }
    if refused.at_live_edge {
        return Err("after a refused catch-up, Live is not offered".into());
    }
    // And Live leaves that too.
    let before = seen(base).await?.len();
    cmds::seek_live().await?;
    let back = await_state(Duration::from_secs(25), |s| s.state == PlaybackState::Playing && s.at_live_edge).await;
    if back.state != PlaybackState::Playing || !back.at_live_edge {
        return Err(format!(
            "Live after a refused catch-up: {:?}, at the live edge: {}",
            back.state, back.at_live_edge
        ));
    }
    let after = seen(base).await?;
    if !after[before..].iter().any(|r| r.kind == "live" && r.stream == STREAM.to_string()) {
        return Err("Live after a refused catch-up did not ask for the channel".into());
    }

    Ok(format!(
        "server {} and this computer {}: five minutes ago was asked for as five minutes ago; \
         Live reopened the channel live; the far edge was asked for {}s in; \
         older than the archive said so, and Live left it",
        panel_clock::describe(panel),
        panel_clock::describe(local),
        asked.start.map_or(0, |s| s - (now - ARCHIVE_DAYS * 86_400)),
    ))
}

/// Polls the state feed until `done`, or until `limit`; the last reading
/// either way.
#[cfg(target_os = "linux")]
async fn await_state(
    limit: std::time::Duration,
    done: impl Fn(&zapper_lib::ipc::types::PlayerStateDto) -> bool,
) -> zapper_lib::ipc::types::PlayerStateDto {
    let deadline = std::time::Instant::now() + limit;
    loop {
        let s = zapper_lib::player::api::player_state()
            .await
            .unwrap_or_default();
        if done(&s) || std::time::Instant::now() >= deadline {
            return s;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}
