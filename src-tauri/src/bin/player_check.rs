//! Verifies the player *commands* against a real stream.
//!
//! `surface_check` proves the GL surface draws. This proves the layer above
//! it: that `play` reaches `Playing` with measured dimensions, that a relative
//! seek moves into the buffered past, that `seek_live` comes back to the edge,
//! and that `stop` stops. None of that is unit-testable - the whole point of
//! these commands is the hop onto the GTK main thread and mpv's real answers.
//!
//! Usage: `tools/fake_stream.py &` then `cargo run --bin player_check`. It
//! plays a local fixture (`ZAPPER_FIXTURE_BASE`, default
//! `http://127.0.0.1:8788`, and `ZAPPER_FIXTURE_NAME`, default
//! `sample1080.ts`) on a scratch account: never the account on this machine.
//!
//! Against a real provider only with `ZAPPER_ALLOW_REAL_ACCOUNT=1`, which
//! plays `ZAPPER_STREAM_ID` through the configured account, or with no id
//! asks the server for one, so a stream URL never reaches a command line: an
//! Xtream URL embeds the account username and password, and argv is
//! world-readable through `/proc` and lands verbatim in crash dumps. Many
//! accounts allow only ONE concurrent connection, so that run takes it from
//! whoever is watching; it opens one stream, keeps it for the duration, and
//! exits.
//!
//! Audio is disabled outright (`ao=null`) before anything is loaded, not
//! muted: this runs unattended and must never make noise.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("player_check only runs on Linux/GTK");
    std::process::exit(2);
}

#[cfg(target_os = "linux")]
async fn quiesce() {
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
}

/// Exits without unwinding. Destructors for mpv and GTK objects owned by the
/// main thread cannot run safely from here, and there is nothing left worth
/// cleaning up in a one-shot harness.
#[cfg(target_os = "linux")]
fn finish(code: i32) -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(code)
}

#[cfg(target_os = "linux")]
fn main() {
    use gtk::prelude::GLAreaExt;
    use tauri::Manager;

    tracing_subscriber::fmt::init();

    // A scratch account, and the fixture server rather than a provider,
    // unless a real run was asked for in so many words. Before Tauri starts
    // a thread: this changes the environment.
    if !zapper_lib::harness::isolate("player_check") {
        if std::env::var_os("ZAPPER_FIXTURE_BASE").is_none() {
            std::env::set_var("ZAPPER_FIXTURE_BASE", zapper_lib::harness::FIXTURE_BASE);
        }
        if std::env::var_os("ZAPPER_FIXTURE_NAME").is_none() {
            std::env::set_var("ZAPPER_FIXTURE_NAME", "sample1080.ts");
        }
    }
    if let Ok(base) = std::env::var("ZAPPER_FIXTURE_BASE") {
        if let Err(e) = zapper_lib::harness::local_url(&base) {
            eprintln!("FAIL: ZAPPER_FIXTURE_BASE: {e}");
            std::process::exit(2);
        }
    }

    tauri::Builder::default()
        .setup(move |app| {
            let window = app.get_webview_window("main").unwrap();
            let ov = zapper_lib::surface::linux::overlay::install(&window)?;
            let player = zapper_lib::surface::linux::render::new_player(&ov.gl_area)?;
            // Kill the audio path entirely before anything is loaded. Muting
            // or zeroing the volume still opens the device and still leaks
            // sound on any path that resets it; `ao=null` cannot.
            player.set_property("ao", "null")?;

            // Nothing drives the GLArea until mpv's update callback fires, and
            // if mpv never decodes, that never happens. Ticking keeps the
            // render count honest about whether the surface works, which is a
            // separate failure from whether the stream works.
            let tick = ov.gl_area.clone();
            gtk::glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
                tick.queue_render();
                gtk::glib::ControlFlow::Continue
            });

            // A hung open must not leave a GUI process on the user's session.
            gtk::glib::timeout_add_seconds_local(240, || {
                eprintln!("FAIL: the run did not finish within 240s");
                std::process::exit(1);
            });

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let verdict = run(&handle).await;
                // Let the GTK render loop finish the frame it is in before the
                // process goes away. Exiting straight out of this task tears
                // mpv's render context down underneath a live GL callback,
                // which segfaults and leaves a core dump - and that dump holds
                // the stream URL, credentials and all.
                quiesce().await;
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
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("player_check failed to run");
}

/// Seconds of playback before the back-seek, so there is a past to seek into.
#[cfg(target_os = "linux")]
const SETTLE: u64 = 14;

/// How far back to seek. Must be comfortably inside `SETTLE`, and well past
/// the live cushion, so that `seek_live` has real distance to cover rather
/// than finding itself already close enough.
#[cfg(target_os = "linux")]
const SEEK_BACK: f64 = -10.0;

/// What counts as "at the live edge": `seek_live` parks playback on the
/// cushion behind the newest data, give or take the couple of seconds the
/// seekable end jumps by as chunks arrive.
#[cfg(target_os = "linux")]
const LIVE: f64 = zapper_lib::ipc::player_cmds::LIVE_CUSHION + 2.5;

/// How long to watch for a stall after `seek_live`.
#[cfg(target_os = "linux")]
const AFTER_LIVE: u64 = 15;

/// Polls the player for `for_how_long`, counting the times playback stopped
/// to wait for the cache (each one a visible stutter), and the thinnest the
/// buffer ahead of the playhead got.
#[cfg(target_os = "linux")]
async fn watch_for_stalls(
    app: &tauri::AppHandle,
    for_how_long: std::time::Duration,
) -> Result<(u32, f64), String> {
    use zapper_lib::ipc::player_cmds as cmds;
    let until = std::time::Instant::now() + for_how_long;
    let (mut stalls, mut was, mut thinnest) = (0u32, false, f64::INFINITY);
    while std::time::Instant::now() < until {
        let s = cmds::player_stats(app.clone()).await?;
        let now = s.buffering.unwrap_or(false);
        if now && !was {
            stalls += 1;
        }
        was = now;
        if let Some(ahead) = s.cache_duration {
            thinnest = thinnest.min(ahead);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    println!("[after-live] stalls={stalls} thinnest_ahead={thinnest:.2}s");
    Ok((stalls, thinnest))
}

#[cfg(target_os = "linux")]
async fn run(app: &tauri::AppHandle) -> Result<String, String> {
    use std::time::Duration;
    use tokio::time::sleep;
    use zapper_lib::ipc::player_cmds as cmds;
    use zapper_lib::ipc::types::PlaybackState;

    let stream_id = resolve_stream_id().await?;
    // Printed so a re-run can pass ZAPPER_STREAM_ID and skip the listing. An
    // id is not a credential; the URL that carries one is never printed.
    println!("[open] stream_id={stream_id}");

    // --- play -------------------------------------------------------------
    cmds::play(app.clone(), stream_id).await?;
    let opened = await_state(Duration::from_secs(25), |s| {
        !matches!(s.state, PlaybackState::Opening)
    })
    .await;
    if opened.state != PlaybackState::Playing {
        return Err(format!(
            "play() ended in {:?}: {}",
            opened.state,
            opened.error.as_deref().unwrap_or("no reason given")
        ));
    }

    // Dimensions are polled after the first frame, so give the ticker a beat.
    let measured = await_state(Duration::from_secs(10), |s| s.width.is_some()).await;
    let (w, h) = match (measured.width, measured.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w, h),
        _ => return Err("Playing, but mpv reported no video dimensions".into()),
    };
    note(app, "playing").await;

    // --- seek back --------------------------------------------------------
    sleep(Duration::from_secs(SETTLE)).await;
    note(app, "settled").await;
    let before = cmds::player_state().await?;
    // Tuning in lands on the server's connect burst — the oldest of what it
    // already held — and must be brought up to live once that has arrived.
    // Run against `fake_stream.py --burst 20` to exercise it; on an even
    // feed it is near live from the start and this holds trivially.
    if before.behind_live > LIVE {
        return Err(format!(
            "{SETTLE}s after tuning in, still {:.1}s behind live (the connect burst was never caught up)",
            before.behind_live
        ));
    }
    if before.buffered_back < 4.0 {
        return Err(format!(
            "only {:.1}s of scrollback after {SETTLE}s of playback; nothing to seek into",
            before.buffered_back
        ));
    }
    cmds::seek_relative(app.clone(), SEEK_BACK).await?;
    let back = await_state(Duration::from_secs(8), |s| {
        s.behind_live >= before.behind_live + 3.0
    })
    .await;
    if back.behind_live <= before.behind_live + 3.0 {
        return Err(format!(
            "a {SEEK_BACK}s seek moved behind_live from {:.1}s only to {:.1}s",
            before.behind_live, back.behind_live
        ));
    }
    if back.behind_live <= 0.0 {
        return Err("behind_live is still 0 after seeking into the past".into());
    }
    note(app, "seeked-back").await;

    // --- back to live -----------------------------------------------------
    cmds::seek_live(app.clone()).await?;
    let live = await_state(Duration::from_secs(10), |s| s.behind_live < LIVE).await;
    note(app, "seeked-live").await;
    if live.behind_live >= LIVE {
        return Err(format!(
            "seek_live left {:.1}s behind the edge (was {:.1}s)",
            live.behind_live, back.behind_live
        ));
    }

    // --- stays smooth after reaching live ---------------------------------
    //
    // Arriving at the edge is only half of it. Landing with nothing buffered
    // ahead plays for a few seconds and then stalls to refill. Watched for
    // long enough to see the stall a thin buffer produces.
    let (stalls, thinnest) = watch_for_stalls(app, Duration::from_secs(AFTER_LIVE)).await?;
    note(app, "after-live").await;
    if stalls > 0 {
        return Err(format!(
            "playback stalled to buffer {stalls} time(s) in the {AFTER_LIVE}s after seek_live \
             (thinnest buffer ahead {thinnest:.1}s)"
        ));
    }

    // --- stop -------------------------------------------------------------
    cmds::stop(app.clone()).await?;
    let stopped = cmds::player_state().await?;
    if stopped.state != PlaybackState::Stopped {
        return Err(format!("stop() left the player in {:?}", stopped.state));
    }

    Ok(format!(
        "stream {stream_id} played {w}x{h} {} @ {}, \
         seek {SEEK_BACK}s took behind_live {:.1}s -> {:.1}s, \
         seek_live returned it to {:.1}s, no stall in the {AFTER_LIVE}s after \
         (thinnest buffer ahead {thinnest:.1}s), stop() reached Stopped",
        measured.codec.as_deref().unwrap_or("codec unknown"),
        measured
            .fps
            .map(|f| format!("{f:.3}fps"))
            .unwrap_or_else(|| "fps unknown".into()),
        before.behind_live,
        back.behind_live,
        live.behind_live,
    ))
}

/// Prints mpv's raw transport properties for one stage of the run.
///
/// The seek behaviour of a live MPEG-TS cache is not documented in a way that
/// matches what any given server does, so a failing run has to say what mpv
/// actually reported rather than only that an assertion tripped.
#[cfg(target_os = "linux")]
async fn note(app: &tauri::AppHandle, stage: &str) {
    let (tx, rx) = std::sync::mpsc::channel();
    let posted = app.run_on_main_thread(move || {
        let d = zapper_lib::surface::mpv::with_player(|p| p.diagnostics())
            .unwrap_or_else(|| "no player on the main thread".to_owned());
        let _ = tx.send(d);
    });
    if posted.is_err() {
        return;
    }
    if let Ok(d) = rx.recv_timeout(std::time::Duration::from_secs(5)) {
        println!("[{stage}] {d}");
    }
    if let Ok(s) = zapper_lib::ipc::player_cmds::player_state().await {
        println!(
            "[{stage}] state={:?} behind_live={:.1} buffered_back={:.1}",
            s.state, s.behind_live, s.buffered_back
        );
    }
}

/// Polls the state feed until `done`, or until `limit` elapses. Returns
/// whatever the last reading was either way, so the caller reports the real
/// numbers rather than a timeout.
#[cfg(target_os = "linux")]
async fn await_state(
    limit: std::time::Duration,
    done: impl Fn(&zapper_lib::ipc::types::PlayerStateDto) -> bool,
) -> zapper_lib::ipc::types::PlayerStateDto {
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + limit;
    loop {
        let s = zapper_lib::ipc::player_cmds::player_state()
            .await
            .unwrap_or_default();
        if done(&s) || Instant::now() >= deadline {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// A channel to test with. `ZAPPER_STREAM_ID` wins. Otherwise, against a
/// fixture, any id does (the fixture server ignores it); against a real
/// account — only when allowed — the server's listing is asked for the first
/// playable entry, which keeps credentials out of both argv and the
/// environment.
#[cfg(target_os = "linux")]
async fn resolve_stream_id() -> Result<i64, String> {
    if let Ok(id) = std::env::var("ZAPPER_STREAM_ID") {
        return id
            .trim()
            .parse()
            .map_err(|_| format!("ZAPPER_STREAM_ID is not a number: {id}"));
    }
    if std::env::var_os("ZAPPER_FIXTURE_BASE").is_some() {
        return Ok(1);
    }
    if !zapper_lib::harness::real_account_allowed() {
        return Err("no fixture and no permission to use the real account".into());
    }
    let channels = zapper_lib::ipc::player_cmds::xtream()?
        .channels()
        .await
        .map_err(|e| format!("asking the server for a channel: {e}"))?;
    // A named channel by preference. The first entry in a huge listing is as
    // likely to be a dead novelty feed as anything, and a verification run
    // that fails on a bad channel says nothing about the code.
    let want = std::env::var("ZAPPER_STREAM_NAME").unwrap_or_else(|_| "NEWS".to_owned());
    let want = want.to_uppercase();
    let playable = || channels.iter().filter(|c| !c.is_separator());
    playable()
        .find(|c| c.name.to_uppercase().contains(&want))
        .or_else(|| playable().next())
        .map(|c| c.stream_id)
        .ok_or_else(|| "the server listed no playable channels".into())
}
