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
//! `--episode` plays an episode through `play_episode`
//! (`ZAPPER_FIXTURE_EPISODE` on the fixture server, default `movie.mkv`;
//! `fake_stream.py --series 1` serves one at
//! `series/<user>/<pass>/100111.mkv`) and checks it reports itself as an
//! episode with its length, then `seek_to`, pause and stop.
//!
//! `--up-next` plays episodes the way Up next moves through them (every id
//! plays `ZAPPER_FIXTURE_EPISODE`, so give it the 120 s episode): one
//! played to its end reports itself as `finished`; moving on from it opens
//! the next and marks it watched in the scratch catalogue; moving on from
//! one still playing does the same, once however often it is asked; moving
//! on from one paused opens the next playing; and no
//! second stream lingers on the fixture server (it sends a title as fast as
//! it is read, so this catches a stream left open, not a brief overlap).
//! With `--past-end` the first episode is ended by a skip past its end
//! (`seek_relative`) rather than a seek near it, which must still count as
//! finished. With `--drop`, against `fake_stream.py --vod-cut-at 0.8`, an
//! episode cut short fails with no finished title, and moving on from it is
//! refused.
//!
//! `--subtitles` opens episodes (`ZAPPER_FIXTURE_EPISODE`, default
//! `movie.mkv`, which has English and French subtitles) one after another,
//! each opened afresh: subtitles start off; French chosen on one shows on
//! the next; turned off, they stay off on the next; and the settings'
//! choice, applied as the interface applies it, is what the next shows.
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
        // Zapping to other ids wants each id to name its own fixture.
        if std::env::var_os("ZAPPER_FIXTURE_NAME").is_none() && std::env::var_os("ZAPPER_ZAP_TO").is_none() {
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
                } else if let Some(secs) = args
                    .iter()
                    .position(|a| a == "--keeps-playing")
                    .and_then(|i| args.get(i + 1))
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    run_keeps_playing(&handle, secs, args.iter().any(|a| a == "--after-movie")).await
                } else if args.iter().any(|a| a == "--movie") {
                    run_movie(&handle).await
                } else if args.iter().any(|a| a == "--up-next") {
                    let has = |flag: &str| args.iter().any(|a| a == flag);
                    run_up_next(&handle, has("--drop"), has("--past-end")).await
                } else if args.iter().any(|a| a == "--subtitles") {
                    run_subtitles(&handle).await
                } else if args.iter().any(|a| a == "--episode") {
                    run_episode(&handle).await
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

    // --- zaps -------------------------------------------------------------
    //
    // `ZAPPER_ZAP_TO=2,3` changes channel from the one playing to each id in
    // turn (each id its own fixture, `<id>.ts`), and checks each plays and is
    // still playing past the first-frame deadline: a radio channel (no
    // video) included, which has no frame to show.
    let mut zapped = Vec::new();
    if let Ok(list) = std::env::var("ZAPPER_ZAP_TO") {
        for id in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let id: i64 = id.parse().map_err(|_| format!("ZAPPER_ZAP_TO: {id} is not an id"))?;
            cmds::play(app.clone(), id).await?;
            let opened = await_state(Duration::from_secs(25), |s| s.state != PlaybackState::Opening).await;
            if opened.state != PlaybackState::Playing {
                return Err(format!(
                    "zap to {id} ended in {:?}: {:?} {:?}",
                    opened.state, opened.ending, opened.error
                ));
            }
            // Past the deadline (8 s), and the hold of a start at live.
            sleep(Duration::from_secs(12)).await;
            let later = cmds::player_state().await?;
            note(app, &format!("zapped-{id}")).await;
            if later.state != PlaybackState::Playing || later.stream_id != Some(id) {
                return Err(format!(
                    "zap to {id}: {:?} after 12 s ({:?} {:?})",
                    later.state, later.ending, later.error
                ));
            }
            zapped.push(format!(
                "{id} ({})",
                later.width.map_or("sound only".to_owned(), |w| format!("{w} wide"))
            ));
        }
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
         (thinnest buffer ahead {thinnest:.1}s), zapped to [{}], stop() reached Stopped",
        measured.codec.as_deref().unwrap_or("codec unknown"),
        measured
            .fps
            .map(|f| format!("{f:.3}fps"))
            .unwrap_or_else(|| "fps unknown".into()),
        before.behind_live,
        back.behind_live,
        live.behind_live,
        zapped.join(", "),
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

/// The `--keeps-playing <secs>` run: a channel that must play, and not be
/// stopped as anything (off air above all), for that long. An episode a
/// playlist lists among its channels, say: an MP4 sent fast and then ended.
///
/// With `--after-movie`, a movie (`ZAPPER_FIXTURE_MOVIE`) is played first,
/// taken to a few seconds from its end so that mpv has read all of it, and
/// the channel opened straight from there: the first heartbeats of the
/// channel must not judge the movie's cache as the channel's.
#[cfg(target_os = "linux")]
async fn run_keeps_playing(app: &tauri::AppHandle, secs: u64, after_movie: bool) -> Result<String, String> {
    use std::time::{Duration, Instant};
    use zapper_lib::ipc::player_cmds as cmds;
    use zapper_lib::ipc::types::PlaybackState;

    let mut before = String::new();
    if after_movie {
        cmds::play_movie(app.clone(), 2, None).await?;
        let s = await_state(Duration::from_secs(25), |s| {
            s.state == PlaybackState::Playing && s.on_demand.is_some_and(|o| o.duration.is_some())
        })
        .await;
        let length = s.on_demand.and_then(|o| o.duration).ok_or("the movie never played")?;
        cmds::seek_to(app.clone(), (length - 6.0).max(0.0)).await?;
        let near = await_state(Duration::from_secs(10), |s| {
            s.on_demand.is_some_and(|o| o.position >= length - 8.0)
        })
        .await;
        // Long enough for its end to have been read, short of playing out.
        tokio::time::sleep(Duration::from_secs(2)).await;
        before = format!(
            "a {length:.0}s movie taken to {:.0}s, then ",
            near.on_demand.map_or(-1.0, |o| o.position)
        );
        println!("[keeps-playing] {before}the channel");
    }

    let stream_id = resolve_stream_id().await?;
    let started = Instant::now();
    cmds::play(app.clone(), stream_id).await?;
    let mut playing_at = None;
    while started.elapsed() < Duration::from_secs(secs) {
        let s = cmds::player_state().await.unwrap_or_default();
        if s.state == PlaybackState::Failed || s.state == PlaybackState::Stopped {
            let kind = s
                .ending
                .and_then(|e| serde_json::to_value(e.kind).ok())
                .and_then(|v| v.as_str().map(str::to_owned));
            return Err(format!(
                "{:?} as {kind:?} after {:.1}s: {}",
                s.state,
                started.elapsed().as_secs_f64(),
                s.error.as_deref().unwrap_or("no words")
            ));
        }
        if playing_at.is_none() && s.state == PlaybackState::Playing {
            playing_at = Some(started.elapsed());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let Some(at) = playing_at else {
        return Err(format!("never reached Playing in {secs}s"));
    };
    Ok(format!(
        "{before}stream {stream_id} played from {:.1}s and was still playing {secs}s after the open",
        at.as_secs_f64()
    ))
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

/// The `--episode` run: an episode opens as an episode (not a channel, not
/// a movie), reports its length, seeks, pauses and stops.
#[cfg(target_os = "linux")]
async fn run_episode(app: &tauri::AppHandle) -> Result<String, String> {
    use std::time::Duration;
    use zapper_lib::ipc::player_cmds as cmds;
    use zapper_lib::ipc::types::{OnDemandKind, PlaybackState, PlayerStateDto};

    fn at(s: &PlayerStateDto) -> f64 {
        s.on_demand.map_or(-1.0, |o| o.position)
    }

    cmds::play_episode(app.clone(), 100_111, None).await?;
    let opened = await_state(Duration::from_secs(25), |s| {
        s.state == PlaybackState::Playing && s.on_demand.is_some_and(|o| o.duration.is_some())
    })
    .await;
    if opened.state != PlaybackState::Playing {
        return Err(format!(
            "play_episode() ended in {:?}: {}",
            opened.state,
            opened.error.as_deref().unwrap_or("no reason given")
        ));
    }
    if opened.stream_id.is_some() {
        return Err("an episode reported a channel".into());
    }
    let od = opened.on_demand.ok_or("an episode that is playing reported no title")?;
    if (od.kind, od.id) != (OnDemandKind::Episode, 100_111) {
        return Err(format!("an episode reported itself as {:?} {}", od.kind, od.id));
    }
    let length = od.duration.ok_or("an episode that is playing reported no length")?;
    if !(115.0..=125.0).contains(&length) {
        return Err(format!("the episode is about 120s long, but it reported {length}s"));
    }

    cmds::seek_to(app.clone(), 60.0).await?;
    let there = await_state(Duration::from_secs(8), |s| (58.0..=62.0).contains(&at(s))).await;
    if !(58.0..=62.0).contains(&at(&there)) {
        return Err(format!("seek_to(60) landed at {:.1}s", at(&there)));
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
        "a {length}s episode played as an episode, seek_to(60) landed at {:.1}s, paused and stopped",
        at(&there)
    ))
}

/// The `--subtitles` run: subtitles chosen on one title are the standing
/// choice for the next ones opened (not Up next: a title opened afresh),
/// in the language chosen; turned off, they stay off across an open, even
/// on a file whose first subtitle track is flagged default; and the
/// settings' choice, applied as the interface applies it, is what an open
/// shows.
#[cfg(target_os = "linux")]
async fn run_subtitles(app: &tauri::AppHandle) -> Result<String, String> {
    use std::time::Duration;
    use zapper_lib::ipc::player_cmds as cmds;
    use zapper_lib::ipc::types::{OnDemandKind, PlaybackState, PlayerStateDto, SettingsDto, TrackDto};

    /// Opens episode `id` (the fixture's `movie.mkv`: English and French
    /// subtitles) and returns its subtitle tracks once they are known and
    /// have had a moment to be chosen.
    async fn open(app: &tauri::AppHandle, id: i64) -> Result<Vec<TrackDto>, String> {
        // Stopped first, until the last title's tracks are gone: every id
        // plays the same file, so its tracks cannot tell the two apart. A
        // Stop changes nothing about the subtitles chosen.
        cmds::stop(app.clone()).await?;
        for _ in 0..50 {
            if cmds::player_options().await?.subtitles.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        cmds::play_episode(app.clone(), id, None).await?;
        let playing = |s: &PlayerStateDto| {
            s.state == PlaybackState::Playing
                && s.on_demand.is_some_and(|o| o.kind == OnDemandKind::Episode && o.id == id)
        };
        let s = await_state(Duration::from_secs(25), playing).await;
        if !playing(&s) {
            return Err(format!(
                "episode {id} ended in {:?}: {}",
                s.state,
                s.error.as_deref().unwrap_or("no reason given")
            ));
        }
        for _ in 0..75 {
            if !cmds::player_options().await?.subtitles.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        // The choice is made on the heartbeat after the tracks are known.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let subtitles = cmds::player_options().await?.subtitles;
        if subtitles.len() < 2 {
            return Err(format!("episode {id} listed {} subtitle tracks, not 2", subtitles.len()));
        }
        Ok(subtitles)
    }
    fn shown(tracks: &[TrackDto]) -> Option<String> {
        tracks.iter().find(|t| t.selected).map(|t| t.lang.clone().unwrap_or_else(|| "?".into()))
    }

    // Off, as a fresh install has them: none shown, though the file's first
    // track is flagged default.
    let first = open(app, 100_111).await?;
    if let Some(lang) = shown(&first) {
        return Err(format!("subtitles were off, but the first title showed {lang}"));
    }

    // French, chosen by hand: the next title shows French.
    let french = first
        .iter()
        .find(|t| t.lang.as_deref() == Some("fra"))
        .ok_or("the fixture has no French subtitles")?
        .id;
    cmds::set_subtitle_track(app.clone(), Some(french)).await?;
    let second = open(app, 100_112).await?;
    if shown(&second).as_deref() != Some("fra") {
        return Err(format!("French was chosen, but the next title showed {:?}", shown(&second)));
    }

    // Off, chosen by hand: the next stays off.
    cmds::set_subtitle_track(app.clone(), None).await?;
    let third = open(app, 100_113).await?;
    if let Some(lang) = shown(&third) {
        return Err(format!("subtitles were turned off, but the next title showed {lang}"));
    }

    // The settings' choice, as saved settings reach the player.
    let settings = SettingsDto {
        subtitles_on: true,
        subtitle_language: Some("eng".into()),
        ..SettingsDto::default()
    };
    cmds::apply_playback_settings(app.clone(), settings).await?;
    let fourth = open(app, 100_114).await?;
    if shown(&fourth).as_deref() != Some("eng") {
        return Err(format!("the settings said English, but the title showed {:?}", shown(&fourth)));
    }

    cmds::stop(app.clone()).await?;
    Ok("off by default though a track is flagged default; French chosen showed French on the next \
        title; off chosen stayed off on the next; the settings' English showed English"
        .into())
}

/// The `--up-next` run; see the top of the file.
#[cfg(target_os = "linux")]
async fn run_up_next(app: &tauri::AppHandle, drop: bool, past_end: bool) -> Result<String, String> {
    use std::time::Duration;
    use zapper_lib::ipc::player_cmds as cmds;
    use zapper_lib::ipc::types::{OnDemandKind, PlaybackState, PlayerStateDto};

    const A: i64 = 100_111;
    const B: i64 = 100_112;
    const C: i64 = 100_113;
    const D: i64 = 100_114;

    fn episode(o: Option<zapper_lib::ipc::types::OnDemandDto>) -> Option<i64> {
        o.filter(|o| o.kind == OnDemandKind::Episode).map(|o| o.id)
    }
    fn at(s: &PlayerStateDto) -> f64 {
        s.on_demand.map_or(-1.0, |o| o.position)
    }
    let playing = |id: i64| {
        move |s: &PlayerStateDto| {
            s.state == PlaybackState::Playing
                && episode(s.on_demand) == Some(id)
                && s.on_demand.is_some_and(|o| o.duration.is_some())
        }
    };
    let failure = |what: &str, s: &PlayerStateDto| {
        format!("{what} ended in {:?}: {}", s.state, s.error.as_deref().unwrap_or("no reason given"))
    };

    cmds::play_episode(app.clone(), A, None).await?;
    let opened = await_state(Duration::from_secs(25), playing(A)).await;
    if !playing(A)(&opened) {
        return Err(failure("play_episode(A)", &opened));
    }

    if drop {
        // Past what the server sends before it cuts the file short.
        cmds::seek_to(app.clone(), 90.0).await?;
        let ended = await_state(Duration::from_secs(60), |s| {
            matches!(s.state, PlaybackState::Failed | PlaybackState::Stopped)
        })
        .await;
        if ended.state != PlaybackState::Failed {
            return Err(format!(
                "an episode cut short at {:.0}s ended in {:?}, not as a failure",
                at(&ended),
                ended.state
            ));
        }
        if ended.finished.is_some() {
            return Err("an episode cut short was reported as finished".into());
        }
        if cmds::play_next_episode(app.clone(), A, B).await.is_ok() {
            return Err("moving on from an episode cut short was not refused".into());
        }
        let after = cmds::player_state().await?;
        if after.state != PlaybackState::Failed || episode(after.on_demand) != Some(A) {
            return Err(format!("a refused move on left the player in {:?}", after.state));
        }
        cmds::stop(app.clone()).await?;
        return Ok(format!(
            "an episode cut short failed ({}), with no finished title, and moving on was refused",
            ended.error.as_deref().unwrap_or("no reason")
        ));
    }

    let port = fixture_port()?;
    let streams = StreamWatch::start(port);

    // Played to its end: from near it, or by skipping past it (the
    // credits), which ends the file before a heartbeat reads the new place.
    if past_end {
        cmds::seek_to(app.clone(), 85.0).await?;
        let there = await_state(Duration::from_secs(8), |s| (84.0..=90.0).contains(&at(s))).await;
        if !(84.0..=90.0).contains(&at(&there)) {
            return Err(format!("seek_to(85) left A at {:.1}s", at(&there)));
        }
        cmds::seek_relative(app.clone(), 60.0).await?;
    } else {
        cmds::seek_to(app.clone(), 115.0).await?;
    }
    let ended = await_state(Duration::from_secs(30), |s| s.state != PlaybackState::Playing).await;
    if ended.state != PlaybackState::Stopped {
        return Err(failure("an episode played to its end", &ended));
    }
    if episode(ended.finished) != Some(A) {
        return Err(format!("an episode played to its end reported {:?} as finished", ended.finished));
    }

    // Moving on from it.
    cmds::play_next_episode(app.clone(), A, B).await?;
    let next = await_state(Duration::from_secs(25), playing(B)).await;
    if !playing(B)(&next) {
        return Err(failure("play_next_episode(A, B)", &next));
    }
    if next.finished.is_some() {
        return Err("the next episode opened with a finished title still reported".into());
    }
    if !watched_soon(A).await? {
        return Err("the episode moved on from was not marked watched".into());
    }

    // Moving on from one still playing, asked twice: opened once.
    cmds::set_subtitle_track(app.clone(), Some(1)).await?;
    cmds::seek_to(app.clone(), 105.0).await?;
    let near = await_state(Duration::from_secs(8), |s| at(s) >= 104.0).await;
    if at(&near) < 104.0 || near.state != PlaybackState::Playing {
        return Err(format!("seek_to(105) left B at {:.1}s, {:?}", at(&near), near.state));
    }
    // At once, as a countdown running out under a press would.
    let (first, second) = tokio::join!(
        cmds::play_next_episode(app.clone(), B, C),
        cmds::play_next_episode(app.clone(), B, C)
    );
    match (first, second) {
        (Ok(()), Err(_)) | (Err(_), Ok(())) => {}
        (Ok(()), Ok(())) => return Err("moving on twice from the same episode opened it twice".into()),
        (Err(e), Err(_)) => return Err(format!("play_next_episode(B, C) was refused: {e}")),
    }
    let last = await_state(Duration::from_secs(25), playing(C)).await;
    if !playing(C)(&last) {
        return Err(failure("play_next_episode(B, C)", &last));
    }
    if !watched_soon(B).await? {
        return Err("the episode moved on from while playing was not marked watched".into());
    }
    // The subtitles chosen on B are shown on C, once its tracks are known.
    let mut subtitles = None;
    for _ in 0..25 {
        let o = cmds::player_options().await?;
        if let Some(t) = o.subtitles.iter().find(|t| t.selected) {
            subtitles = t.lang.clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    if subtitles.as_deref() != Some("eng") {
        return Err(format!("the next episode's subtitles were {subtitles:?}, not the English chosen"));
    }

    // Moving on from one paused (Play now on a paused picture): the next
    // opens playing, not paused as the last one was left.
    cmds::toggle_pause(app.clone()).await?;
    let paused = await_state(Duration::from_secs(5), |s| s.state == PlaybackState::Paused).await;
    if paused.state != PlaybackState::Paused || episode(paused.on_demand) != Some(C) {
        return Err(format!("toggle_pause() left C in {:?}", paused.state));
    }
    cmds::play_next_episode(app.clone(), C, D).await?;
    let resumed = await_state(Duration::from_secs(25), playing(D)).await;
    if !playing(D)(&resumed) {
        return Err(failure("play_next_episode(C, D) from a paused C", &resumed));
    }

    cmds::stop(app.clone()).await?;
    let most = streams.stop()?;
    if most >= 2 {
        return Err(format!("a second stream lingered on the fixture server ({most} open)"));
    }
    Ok(format!(
        "A {} as finished; A to B opened B and marked A watched; B to C at {:.0}s \
         opened C once, marked B watched and kept the subtitles; a paused C to D opened D \
         playing; no second stream lingered",
        if past_end { "skipped past its end" } else { "played to its end" },
        at(&near)
    ))
}

/// Whether episode `id` is marked watched in the scratch catalogue within a
/// few seconds: the mark is written off the main thread.
#[cfg(target_os = "linux")]
async fn watched_soon(id: i64) -> Result<bool, String> {
    use rusqlite::OptionalExtension;
    for _ in 0..25 {
        let watched = zapper_lib::catalog::db::with_db(|c| {
            Ok(c.query_row(
                "SELECT watched_at IS NOT NULL FROM vod_marks WHERE kind = 'episode' AND item_id = ?1",
                [id],
                |r| r.get::<_, bool>(0),
            )
            .optional()?)
        })
        .map_err(|e| format!("reading the scratch catalogue: {e}"))?;
        if watched == Some(true) {
            return Ok(true);
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    Ok(false)
}

/// The fixture server's port, from `ZAPPER_FIXTURE_BASE`.
#[cfg(target_os = "linux")]
fn fixture_port() -> Result<u16, String> {
    let base = std::env::var("ZAPPER_FIXTURE_BASE").map_err(|_| "no ZAPPER_FIXTURE_BASE")?;
    base.trim_end_matches('/')
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .ok_or_else(|| format!("no port in ZAPPER_FIXTURE_BASE ({base})"))
}

/// Watches the fixture server's account API for a second stream left open,
/// until stopped. Only a lingering one: the server sends a title as fast as
/// it is read, so a stream is open for moments and a brief overlap between
/// two would pass unseen. Two are believed only when two readings a third
/// of a second apart agree, since the server's thread sending a stream that
/// was just closed notices a moment later (as `fake_stream.py` allows for).
#[cfg(target_os = "linux")]
struct StreamWatch {
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: std::thread::JoinHandle<Result<i64, String>>,
}

#[cfg(target_os = "linux")]
impl StreamWatch {
    fn start(port: u16) -> Self {
        use std::sync::atomic::Ordering;
        let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let still = running.clone();
        let thread = std::thread::spawn(move || {
            let mut most = 0;
            let mut last = 0;
            while still.load(Ordering::Acquire) {
                // A reading that failed is not a reading of none.
                let now = active_streams(port)
                    .ok_or("the fixture server's account API could not be read")?;
                most = most.max(now.min(last));
                last = now;
                std::thread::sleep(std::time::Duration::from_millis(350));
            }
            Ok(most)
        });
        Self { running, thread }
    }

    /// The most streams it saw open at once, by two readings; an error if
    /// the server could not be asked.
    fn stop(self) -> Result<i64, String> {
        self.running.store(false, std::sync::atomic::Ordering::Release);
        self.thread
            .join()
            .map_err(|_| "the stream watch panicked".to_owned())?
    }
}

/// `active_cons` from the fixture server's account API, over a plain socket.
#[cfg(target_os = "linux")]
fn active_streams(port: u16) -> Option<i64> {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok()?;
    s.write_all(b"GET /player_api.php?username=u&password=p HTTP/1.0\r\nHost: x\r\n\r\n").ok()?;
    let mut body = String::new();
    s.read_to_string(&mut body).ok()?;
    let v: serde_json::Value = serde_json::from_str(&body[body.find("\r\n\r\n")? + 4..]).ok()?;
    let n = v.get("user_info")?.get("active_cons")?;
    n.as_i64().or_else(|| n.as_str()?.parse().ok())
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
