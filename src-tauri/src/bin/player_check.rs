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
//! `--movie` plays a movie instead (`ZAPPER_FIXTURE_MOVIE` on the fixture
//! server, default `movie.mp4`; `fake_stream.py --movies 2` serves a film
//! with ranges at `movie/<user>/<pass>/2.mp4`) and checks its clock,
//! `seek_to`, a relative seek past what was buffered, pause and stop.
//!
//! `--ending <kind>` plays a channel that is expected to stop by itself, and
//! checks it stops the way the app explains it (`offAir`, `dropped`, …), as
//! the ending kinds are spelled to the interface. Against
//! `fake_stream.py --placeholder` (`offAir`) it also checks the stop came
//! within `ending::OFF_AIR_WINDOW` of the open, said how long the clip was,
//! never skipped ahead or reconnected, and was written against the channel
//! as off air.
//!
//! `ZAPPER_LIVE_START` (`wait`, `catchUp`, `stay`) chooses how the channel
//! starts, in any run; unset, as a fresh install does.
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
                let args: Vec<String> = std::env::args().collect();
                let ending = args
                    .iter()
                    .position(|a| a == "--ending")
                    .and_then(|i| args.get(i + 1).cloned());
                let verdict = if let Err(e) = live_start_from_env(&handle).await {
                    Err(e)
                } else if let Some(kind) = ending {
                    run_ending(&handle, &kind).await
                } else if args.iter().any(|a| a == "--movie") {
                    run_movie(&handle).await
                } else {
                    run(&handle).await
                };
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

/// "When a channel opens" from `ZAPPER_LIVE_START` (`wait`, `catchUp`,
/// `stay`), for any run; left as a fresh install has it when unset.
#[cfg(target_os = "linux")]
async fn live_start_from_env(app: &tauri::AppHandle) -> Result<(), String> {
    use zapper_lib::ipc::types::LiveStart;
    let Ok(mode) = std::env::var("ZAPPER_LIVE_START") else {
        return Ok(());
    };
    let mut settings = zapper_lib::catalog::settings::load_or_default();
    settings.live_start = LiveStart::from(mode.clone());
    println!("[start] live start: {:?} (from {mode:?})", settings.live_start);
    zapper_lib::ipc::player_cmds::apply_playback_settings(app.clone(), settings).await
}

/// The `--ending <kind>` run: a channel that stops by itself, and how.
#[cfg(target_os = "linux")]
async fn run_ending(app: &tauri::AppHandle, want: &str) -> Result<String, String> {
    use std::time::{Duration, Instant};
    use zapper_lib::ipc::ending::{EndingKind, OFF_AIR_REASON, OFF_AIR_WINDOW};
    use zapper_lib::ipc::player_cmds as cmds;
    use zapper_lib::ipc::types::PlaybackState;

    let stream_id = resolve_stream_id().await?;
    let started = Instant::now();
    cmds::play(app.clone(), stream_id).await?;

    // Watched closely until it stops: anything that would show a skip ahead
    // (a reported jump, a playhead far from where the stream began) or a
    // reconnect is noted on the way.
    let mut jumped = None;
    let mut reconnected = false;
    let mut playing_at = None;
    let ended = loop {
        let s = cmds::player_state().await.unwrap_or_default();
        jumped = jumped.or(s.live_jump);
        reconnected |= s.reconnect.is_some();
        if playing_at.is_none() && matches!(s.state, PlaybackState::Playing | PlaybackState::Paused) {
            playing_at = Some(started.elapsed());
        }
        if s.state == PlaybackState::Failed {
            break s;
        }
        if started.elapsed() > Duration::from_secs(90) {
            return Err(format!("still {:?} after 90s; expected it to end as {want}", s.state));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let after = started.elapsed();
    let e = ended.ending.ok_or_else(|| {
        format!("failed with no ending: {}", ended.error.as_deref().unwrap_or("no words"))
    })?;
    let kind = serde_json::to_value(e.kind).map_err(|x| x.to_string())?;
    let kind = kind.as_str().unwrap_or("?").to_owned();
    println!(
        "[ending] {kind} after {:.1}s (first picture moving at {}), clip {:?}s, error {:?}",
        after.as_secs_f64(),
        playing_at.map_or("never".into(), |t| format!("{:.1}s", t.as_secs_f64())),
        e.clip_secs,
        ended.error,
    );
    if kind != want {
        return Err(format!("ended as {kind}, expected {want}"));
    }

    // It stays stopped: nothing reconnects by itself, nothing plays out.
    tokio::time::sleep(Duration::from_secs(12)).await;
    let later = cmds::player_state().await?;
    reconnected |= later.reconnect.is_some();
    if later.state != PlaybackState::Failed || later.ending.map(|l| l.kind) != Some(e.kind) {
        return Err(format!("12s later it was {:?} ({:?})", later.state, later.ending.map(|l| l.kind)));
    }

    if e.kind != EndingKind::OffAir {
        return Ok(format!(
            "ended as {kind} after {:.1}s, and stayed that way{}",
            after.as_secs_f64(),
            if reconnected { ", after reconnecting" } else { "" }
        ));
    }

    if after.as_secs_f64() > OFF_AIR_WINDOW {
        return Err(format!("off air only after {:.1}s, past the {OFF_AIR_WINDOW}s window", after.as_secs_f64()));
    }
    if !e.clip_secs.is_some_and(|c| (590..=610).contains(&c)) {
        return Err(format!("the clip is 600s, but the ending says {:?}", e.clip_secs));
    }
    if let Some(j) = jumped {
        return Err(format!("it skipped {:.1}s ahead before stopping", j.seconds));
    }
    if reconnected {
        return Err("it reconnected by itself".into());
    }
    // Written against the channel, for the list to dim it.
    let mut reason = None;
    for _ in 0..20 {
        reason = zapper_lib::catalog::db::with_db(|c| {
            use rusqlite::OptionalExtension;
            Ok(c.query_row(
                "SELECT fail_reason, failed_at FROM channel_quality WHERE stream_id = ?1",
                [stream_id],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<i64>>(1)?)),
            )
            .optional()?)
        })
        .map_err(|x| x.to_string())?;
        if reason.as_ref().is_some_and(|(why, _)| why.is_some()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    match reason {
        Some((Some(why), Some(at))) if why == OFF_AIR_REASON => {
            println!("[ending] channel_quality: fail_reason={why:?} failed_at={at}");
        }
        other => return Err(format!("the channel's record says {other:?}, not off air")),
    }
    Ok(format!(
        "off air {:.1}s after the open (window {OFF_AIR_WINDOW}s), a {}s clip, \
         no skip ahead reported, no reconnect, \
         written against the channel as {OFF_AIR_REASON:?}, still stopped 12s later",
        after.as_secs_f64(),
        e.clip_secs.unwrap_or(0),
    ))
}

/// The `--movie` run: a title opens, reports its length, seeks both ways,
/// pauses and stops.
#[cfg(target_os = "linux")]
async fn run_movie(app: &tauri::AppHandle) -> Result<String, String> {
    use std::time::Duration;
    use zapper_lib::ipc::player_cmds as cmds;
    use zapper_lib::ipc::types::{PlaybackState, PlayerStateDto};

    // The title's own clock, which is what a seek moves.
    fn at(s: &PlayerStateDto) -> f64 {
        s.on_demand.map_or(-1.0, |o| o.position)
    }

    cmds::play_movie(app.clone(), 1, None).await?;
    let opened = await_state(Duration::from_secs(25), |s| {
        s.state == PlaybackState::Playing && s.on_demand.is_some_and(|o| o.duration.is_some())
    })
    .await;
    if opened.state != PlaybackState::Playing {
        return Err(format!(
            "play_movie() ended in {:?}: {}",
            opened.state,
            opened.error.as_deref().unwrap_or("no reason given")
        ));
    }
    if opened.stream_id.is_some() {
        return Err("a movie reported a channel".into());
    }
    let length = opened
        .on_demand
        .and_then(|o| o.duration)
        .ok_or("a movie that is playing reported no length")?;
    if !(115.0..=125.0).contains(&length) {
        return Err(format!("the film is about 120s long, but it reported {length}s"));
    }

    cmds::seek_to(app.clone(), 60.0).await?;
    let there = await_state(Duration::from_secs(8), |s| (58.0..=63.0).contains(&at(s))).await;
    if !(58.0..=63.0).contains(&at(&there)) {
        return Err(format!("seek_to(60) landed at {:.1}s", at(&there)));
    }

    cmds::seek_relative(app.clone(), -10.0).await?;
    let back = await_state(Duration::from_secs(8), |s| at(s) < at(&there) - 8.0).await;
    if at(&back) >= at(&there) - 8.0 {
        return Err(format!(
            "a 10s seek back moved {:.1}s to {:.1}s",
            at(&there),
            at(&back)
        ));
    }

    // Before what the demuxer kept: a film can be read from any part of it,
    // so this is not clamped to the buffer as a live stream's seek is.
    cmds::seek_to(app.clone(), 30.0).await?;
    await_state(Duration::from_secs(8), |s| (28.0..=33.0).contains(&at(s))).await;
    cmds::seek_relative(app.clone(), -20.0).await?;
    let early = await_state(Duration::from_secs(8), |s| at(s) < 15.0).await;
    if at(&early) >= 15.0 {
        return Err(format!("a 20s seek back from 30s landed at {:.1}s", at(&early)));
    }

    cmds::toggle_pause(app.clone()).await?;
    let paused = await_state(Duration::from_secs(5), |s| s.state == PlaybackState::Paused).await;
    if paused.state != PlaybackState::Paused {
        return Err(format!("toggle_pause() left the player in {:?}", paused.state));
    }
    cmds::toggle_pause(app.clone()).await?;

    cmds::stop(app.clone()).await?;
    let stopped = cmds::player_state().await?;
    if stopped.state != PlaybackState::Stopped || stopped.on_demand.is_some() {
        return Err(format!("stop() left the player in {:?}", stopped.state));
    }
    Ok(format!(
        "a {length}s movie played, seek_to(60) landed at {:.1}s, -10s moved to {:.1}s, \
         -20s from 30s reached {:.1}s, paused and stopped",
        at(&there),
        at(&back),
        at(&early)
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
