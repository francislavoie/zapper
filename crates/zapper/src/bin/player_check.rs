//! Verifies the player *commands* against a real stream.
//!
//! `surface_check` proves the GL surface draws. This proves the layer above
//! it: that `play` reaches `Playing` with measured dimensions, that a relative
//! seek moves into the buffered past, that `seek_live` comes back to the edge,
//! and that `stop` stops. None of that is unit-testable - the whole point of
//! these commands is mpv's real answers.
//!
//! The player is mpv in a window of its own (`surface::host`), with no
//! webview: run it inside a desktop session, the headless gnome-shell for
//! anything unattended.
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
//! `--trickplay-vod` (with `fake_stream.py --movies 2` and
//! `ZAPPER_FIXTURE_MOVIE=movie/u/p/2.mp4`, the 120 s 1080p film) checks the
//! previews on a title's seek bar. With mpv's buffer cut to a few megabytes,
//! so it holds about half a minute ahead: every slot from the playhead to
//! the buffered end has a frame and nothing past that end has one;
//! `thumbnail_for_title` answers a JPEG for a filled slot and nothing for an
//! empty one; a seek ahead past the buffer fills the new stretch; the same
//! film opened again keeps them; no excerpt is left in the temporary
//! directory. Then, the whole film buffered, it plays for
//! `ZAPPER_TRICKPLAY_MEASURE_SECS` (default 20) three times, with the
//! worker paused, running flat out (its store emptied every two seconds)
//! and paused again, and prints mpv's dropped frames and how evenly
//! `time-pos` moved in each.
//!
//! `--held` (with `fake_stream.py --movies 2`) opens a movie held at 30 s,
//! as Resume / Start over asks: paused there and staying put; Resume
//! (`set_paused(false)`, sent twice) plays it on from there; held again
//! and then replaced by an open of the same movie, that one plays.
//!
//! `--pause-then-play` (with `fake_stream.py --movies 2`) checks that Play
//! plays even when something was paused: a movie paused and stopped, then
//! another movie; that one paused, then a channel. Each must reach Playing
//! with its clock advancing.
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
//! on from one paused opens the next playing; a next episode left part way
//! (written in the scratch catalogue) opens where it was left; and no
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
//! each opened afresh: subtitles start off (a track chosen, hidden); turned
//! on, off and on again mid-play, and switched from English to French, the
//! episode plays on; French chosen on one shows on the next; turned off,
//! they stay off on the next; and the settings' choice, applied as the
//! interface applies it, is what the next shows. Against `fake_stream.py
//! --keep-alive-timeout 5` the switch comes after the server has closed the
//! connection the episode arrived on.
//!
//! `--find-subtitles` (with `fake_stream.py --movies 2`,
//! `ZAPPER_FIXTURE_MOVIE=movie/u/p/1.mkv`, and `ZAPPER_OPENSUBTITLES_URL` at
//! `tools/fake_opensubtitles.py`) lists movie 1 in the scratch catalogue as
//! "Film 0001" (2021), plays it, finds its English subtitles, is asked to
//! sign in to download one, signs in, downloads it and adds it mid-play:
//! mpv lists it as an external track, chosen, named and shown, draws its
//! lines, and the film plays on without being opened again. A second
//! download of it asks the stand-in nothing. `--find-subtitles
//! --search-only --tmdb <id>` lists movie 1 under that TMDB id and only
//! searches (no sign-in, no download, nothing played), printing the
//! answer's kind and how many it found.
//!
//! `--captions` plays a channel with closed captions (CEA-608 in the
//! picture) that shows one caption top left, one centred at the bottom and
//! one on the right: `ZAPPER_FIXTURE_NAME=captions169.ts` (16:9) or
//! `captions43.ts` (4:3, with bands either side in a wider window), which
//! `tools/make_fixtures.sh` makes. It chooses the captions and checks that
//! mpv draws each on its side and rows of the picture, a side caption's edge
//! inside the picture rather than in a band; that turning them off hides
//! them and keeps the track; and that with no captions track mpv's own
//! placement is back. A caption drawn twice fails it.
//! `ZAPPER_CAPTION_SHOTS=<dir>` saves the window at each caption there.
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
    eprintln!("player_check only runs on Linux");
    std::process::exit(2);
}

#[cfg(target_os = "linux")]
async fn quiesce() {
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
}

/// Exits without unwinding: mpv's threads, its window among them, are still
/// running, and there is nothing left worth cleaning up in a one-shot
/// harness.
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

    // A scratch account, and the fixture server rather than a provider,
    // unless a real run was asked for in so many words. Before any thread
    // starts: this changes the environment.
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

    // A hung open must not leave a window behind. `ZAPPER_CHECK_TIMEOUT_SECS`
    // for a run meant to be longer (`--trickplay-vod` on a 4K film, whose
    // worker paces itself).
    let limit: u64 = std::env::var("ZAPPER_CHECK_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(240);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(limit));
        eprintln!("FAIL: the run did not finish within {limit}s");
        finish(1);
    });

    // mpv in a window of its own, installed as the player. Audio off
    // outright (`ao=null`) before anything is loaded: muting or zeroing the
    // volume still opens the device and still leaks sound on any path that
    // resets it; `ao=null` cannot.
    let opened = zapper_lib::surface::host::open_host(zapper_lib::surface::host::HostOptions {
        title: "zapper player_check".to_owned(),
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
        let args: Vec<String> = std::env::args().collect();
        let ending = args
            .iter()
            .position(|a| a == "--ending")
            .and_then(|i| args.get(i + 1).cloned());
        let verdict = if let Err(e) = live_start_from_env().await {
            Err(e)
        } else if let Some(kind) = ending {
            run_ending(&kind).await
        } else if let Some(secs) = args
            .iter()
            .position(|a| a == "--keeps-playing")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse::<u64>().ok())
        {
            run_keeps_playing(secs, args.iter().any(|a| a == "--after-movie")).await
        } else if args.iter().any(|a| a == "--movie") {
            run_movie().await
        } else if args.iter().any(|a| a == "--trickplay-vod") {
            run_trickplay_vod().await
        } else if args.iter().any(|a| a == "--held") {
            run_held().await
        } else if args.iter().any(|a| a == "--pause-then-play") {
            run_pause_then_play().await
        } else if args.iter().any(|a| a == "--up-next") {
            let has = |flag: &str| args.iter().any(|a| a == flag);
            run_up_next(has("--drop"), has("--past-end")).await
        } else if args.iter().any(|a| a == "--captions") {
            run_captions().await
        } else if args.iter().any(|a| a == "--subtitles") {
            run_subtitles().await
        } else if args.iter().any(|a| a == "--find-subtitles") {
            let tmdb = args
                .iter()
                .position(|a| a == "--tmdb")
                .and_then(|i| args.get(i + 1))
                .and_then(|s| s.parse::<i64>().ok());
            if args.iter().any(|a| a == "--search-only") {
                run_subtitle_search(tmdb.unwrap_or(FILM_TMDB)).await
            } else {
                run_find_subtitles().await
            }
        } else if args.iter().any(|a| a == "--episode") {
            run_episode().await
        } else {
            run().await
        };
        // Let mpv's window finish the frame it is drawing before the process
        // goes away. Exiting in the middle of one tears mpv's video output
        // down underneath it, which can segfault and leave a core dump - and
        // that dump holds the stream URL, credentials and all.
        quiesce().await;
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
const LIVE: f64 = zapper_lib::player::LIVE_CUSHION + 2.5;

/// How long to watch for a stall after `seek_live`.
#[cfg(target_os = "linux")]
const AFTER_LIVE: u64 = 15;

/// Polls the player for `for_how_long`, counting the times playback stopped
/// to wait for the cache (each one a visible stutter), and the thinnest the
/// buffer ahead of the playhead got.
#[cfg(target_os = "linux")]
async fn watch_for_stalls(
    for_how_long: std::time::Duration,
) -> Result<(u32, f64), String> {
    use zapper_lib::player::api as cmds;
    let until = std::time::Instant::now() + for_how_long;
    let (mut stalls, mut was, mut thinnest) = (0u32, false, f64::INFINITY);
    while std::time::Instant::now() < until {
        let s = cmds::player_stats().await?;
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
async fn run() -> Result<String, String> {
    use std::time::Duration;
    use tokio::time::sleep;
    use zapper_lib::player::api as cmds;
    use zapper_lib::ipc::types::PlaybackState;

    let stream_id = resolve_stream_id().await?;
    // Printed so a re-run can pass ZAPPER_STREAM_ID and skip the listing. An
    // id is not a credential; the URL that carries one is never printed.
    println!("[open] stream_id={stream_id}");

    // --- play -------------------------------------------------------------
    cmds::play(stream_id).await?;
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
    note("playing").await;

    // --- seek back --------------------------------------------------------
    sleep(Duration::from_secs(SETTLE)).await;
    note("settled").await;
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
    cmds::seek_relative(SEEK_BACK).await?;
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
    note("seeked-back").await;

    // --- back to live -----------------------------------------------------
    cmds::seek_live().await?;
    let live = await_state(Duration::from_secs(10), |s| s.behind_live < LIVE).await;
    note("seeked-live").await;
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
    let (stalls, thinnest) = watch_for_stalls(Duration::from_secs(AFTER_LIVE)).await?;
    note("after-live").await;
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
            cmds::play(id).await?;
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
            note(&format!("zapped-{id}")).await;
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
    cmds::stop().await?;
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
async fn live_start_from_env() -> Result<(), String> {
    use zapper_lib::ipc::types::LiveStart;
    let Ok(mode) = std::env::var("ZAPPER_LIVE_START") else {
        return Ok(());
    };
    let mut settings = zapper_lib::catalog::settings::load_or_default();
    settings.live_start = LiveStart::from(mode.clone());
    println!("[start] live start: {:?} (from {mode:?})", settings.live_start);
    zapper_lib::player::api::apply_playback_settings(settings).await
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
async fn run_keeps_playing(secs: u64, after_movie: bool) -> Result<String, String> {
    use std::time::{Duration, Instant};
    use zapper_lib::player::api as cmds;
    use zapper_lib::ipc::types::PlaybackState;

    let mut before = String::new();
    if after_movie {
        cmds::play_movie(2, None, None).await?;
        let s = await_state(Duration::from_secs(25), |s| {
            s.state == PlaybackState::Playing && s.on_demand.is_some_and(|o| o.duration.is_some())
        })
        .await;
        let length = s.on_demand.and_then(|o| o.duration).ok_or("the movie never played")?;
        cmds::seek_to((length - 6.0).max(0.0)).await?;
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
    cmds::play(stream_id).await?;
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

/// The `--pause-then-play` run: mpv's pause outlives a file, and a Play the
/// viewer presses must play regardless.
#[cfg(target_os = "linux")]
async fn run_pause_then_play() -> Result<String, String> {
    use std::time::Duration;
    use zapper_lib::player::api as cmds;
    use zapper_lib::ipc::types::{PlaybackState, PlayerStateDto};

    // A title's own clock; a channel's is the picture kept behind the playhead.
    fn clock(s: &PlayerStateDto) -> f64 {
        s.on_demand.map_or(s.buffered_back, |o| o.position)
    }
    async fn plays(what: &str) -> Result<(), String> {
        let s = await_state(Duration::from_secs(25), |s| {
            s.state == PlaybackState::Playing || s.state == PlaybackState::Failed
        })
        .await;
        if s.state != PlaybackState::Playing {
            return Err(format!("{what} ended in {:?}: {}", s.state, s.error.as_deref().unwrap_or("no reason given")));
        }
        let first = clock(&s);
        tokio::time::sleep(Duration::from_millis(2500)).await;
        let s = cmds::player_state().await?;
        let later = clock(&s);
        if s.state != PlaybackState::Playing || later < first + 1.0 {
            return Err(format!("{what} was {:?} with its clock at {first:.1}s then {later:.1}s", s.state));
        }
        println!("[pause-then-play] {what} plays: {first:.1}s -> {later:.1}s");
        Ok(())
    }
    async fn pause(what: &str) -> Result<(), String> {
        cmds::toggle_pause().await?;
        let s = await_state(Duration::from_secs(5), |s| s.state == PlaybackState::Paused).await;
        if s.state != PlaybackState::Paused {
            return Err(format!("{what} would not pause: {:?}", s.state));
        }
        Ok(())
    }

    cmds::play_movie(1, None, None).await?;
    plays("the first movie").await?;
    pause("the first movie").await?;
    cmds::stop().await?;
    cmds::play_movie(2, None, None).await?;
    plays("a movie after a pause and a stop").await?;
    pause("the second movie").await?;
    let stream_id = resolve_stream_id().await?;
    cmds::play(stream_id).await?;
    plays("a channel after a paused movie").await?;
    Ok("a movie played after a paused one was stopped, and a channel after a paused movie".into())
}

/// The `--ending <kind>` run: a channel that stops by itself, and how.
#[cfg(target_os = "linux")]
async fn run_ending(want: &str) -> Result<String, String> {
    use std::time::{Duration, Instant};
    use zapper_lib::ipc::ending::{EndingKind, OFF_AIR_REASON, OFF_AIR_WINDOW};
    use zapper_lib::player::api as cmds;
    use zapper_lib::ipc::types::PlaybackState;

    let stream_id = resolve_stream_id().await?;
    let started = Instant::now();
    cmds::play(stream_id).await?;

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
async fn run_movie() -> Result<String, String> {
    use std::time::Duration;
    use zapper_lib::player::api as cmds;
    use zapper_lib::ipc::types::{PlaybackState, PlayerStateDto};

    // The title's own clock, which is what a seek moves.
    fn at(s: &PlayerStateDto) -> f64 {
        s.on_demand.map_or(-1.0, |o| o.position)
    }

    cmds::play_movie(1, None, None).await?;
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

    cmds::seek_to(60.0).await?;
    let there = await_state(Duration::from_secs(8), |s| (58.0..=63.0).contains(&at(s))).await;
    if !(58.0..=63.0).contains(&at(&there)) {
        return Err(format!("seek_to(60) landed at {:.1}s", at(&there)));
    }

    cmds::seek_relative(-10.0).await?;
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
    cmds::seek_to(30.0).await?;
    await_state(Duration::from_secs(8), |s| (28.0..=33.0).contains(&at(s))).await;
    cmds::seek_relative(-20.0).await?;
    let early = await_state(Duration::from_secs(8), |s| at(s) < 15.0).await;
    if at(&early) >= 15.0 {
        return Err(format!("a 20s seek back from 30s landed at {:.1}s", at(&early)));
    }

    cmds::toggle_pause().await?;
    let paused = await_state(Duration::from_secs(5), |s| s.state == PlaybackState::Paused).await;
    if paused.state != PlaybackState::Paused {
        return Err(format!("toggle_pause() left the player in {:?}", paused.state));
    }
    cmds::toggle_pause().await?;

    cmds::stop().await?;
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

/// The `--held` run; see the top of the file.
#[cfg(target_os = "linux")]
async fn run_held() -> Result<String, String> {
    use std::time::Duration;
    use zapper_lib::player::api as cmds;
    use zapper_lib::ipc::types::{PlaybackState, PlayerStateDto};

    fn at(s: &PlayerStateDto) -> f64 {
        s.on_demand.map_or(-1.0, |o| o.position)
    }
    fn failure(what: &str, s: &PlayerStateDto) -> String {
        format!("{what} ended in {:?}: {}", s.state, s.error.as_deref().unwrap_or("no reason given"))
    }
    async fn held_at_30(what: &str) -> Result<f64, String> {
        cmds::play_movie(2, Some(30.0), Some(true)).await?;
        let s = await_state(Duration::from_secs(5), |s| {
            s.state == PlaybackState::Paused && (28.0..=32.0).contains(&at(s))
        })
        .await;
        if s.state != PlaybackState::Paused || !(28.0..=32.0).contains(&at(&s)) {
            return Err(format!("{what}: {} at {:.1}s", failure("a held open", &s), at(&s)));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        let later = cmds::player_state().await?;
        if later.state != PlaybackState::Paused || (at(&later) - at(&s)).abs() > 0.5 {
            return Err(format!(
                "{what}: held at {:.1}s, then {:?} at {:.1}s",
                at(&s),
                later.state,
                at(&later)
            ));
        }
        Ok(at(&s))
    }
    async fn plays_on(what: &str) -> Result<(f64, f64), String> {
        let s = await_state(Duration::from_secs(25), |s| {
            s.state == PlaybackState::Playing || s.state == PlaybackState::Failed
        })
        .await;
        if s.state != PlaybackState::Playing {
            return Err(failure(what, &s));
        }
        let first = at(&s);
        tokio::time::sleep(Duration::from_millis(2500)).await;
        let s = cmds::player_state().await?;
        if s.state != PlaybackState::Playing || at(&s) < first + 1.0 {
            return Err(format!("{what} was {:?} with its clock at {first:.1}s then {:.1}s", s.state, at(&s)));
        }
        Ok((first, at(&s)))
    }

    let held = held_at_30("the first held open").await?;
    // Resume, sent twice (a double press): asking twice is asking once.
    cmds::set_paused(false).await?;
    cmds::set_paused(false).await?;
    let (from, to) = plays_on("Resume").await?;
    if !(28.0..=34.0).contains(&from) {
        return Err(format!("Resume played on from {from:.1}s, not where it was held"));
    }
    held_at_30("the second held open").await?;
    cmds::play_movie(2, None, None).await?;
    let (again, later) = plays_on("an open after an unanswered held one").await?;
    cmds::stop().await?;
    Ok(format!(
        "held paused at {held:.1}s; Resume played {from:.1}s -> {to:.1}s; held again, the next open \
         played {again:.1}s -> {later:.1}s"
    ))
}

/// The `--episode` run: an episode opens as an episode (not a channel, not
/// a movie), reports its length, seeks, pauses and stops.
#[cfg(target_os = "linux")]
async fn run_episode() -> Result<String, String> {
    use std::time::Duration;
    use zapper_lib::player::api as cmds;
    use zapper_lib::ipc::types::{OnDemandKind, PlaybackState, PlayerStateDto};

    fn at(s: &PlayerStateDto) -> f64 {
        s.on_demand.map_or(-1.0, |o| o.position)
    }

    cmds::play_episode(100_111, None, None).await?;
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

    cmds::seek_to(60.0).await?;
    let there = await_state(Duration::from_secs(8), |s| (58.0..=62.0).contains(&at(s))).await;
    if !(58.0..=62.0).contains(&at(&there)) {
        return Err(format!("seek_to(60) landed at {:.1}s", at(&there)));
    }

    cmds::toggle_pause().await?;
    let paused = await_state(Duration::from_secs(5), |s| s.state == PlaybackState::Paused).await;
    if paused.state != PlaybackState::Paused {
        return Err(format!("toggle_pause() left the player in {:?}", paused.state));
    }
    cmds::toggle_pause().await?;

    cmds::stop().await?;
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
/// on a file whose first subtitle track is flagged default, with the track
/// chosen and hidden; and the settings' choice, applied as the interface
/// applies it, is what an open shows. Mid-play, turning them off and on
/// again (the toggle) and switching from English to French (the menu) keep
/// the title playing.
#[cfg(target_os = "linux")]
async fn run_subtitles() -> Result<String, String> {
    use std::time::Duration;
    use zapper_lib::player::api as cmds;
    use zapper_lib::ipc::types::{
        OnDemandKind, PlaybackState, PlayerOptionsDto, PlayerStateDto, SettingsDto,
    };

    /// Opens episode `id` (the fixture's `movie.mkv`: English and French
    /// subtitles) and returns its options once its tracks are known and
    /// have had a moment to be chosen.
    async fn open(id: i64) -> Result<PlayerOptionsDto, String> {
        // Stopped first, until the last title's tracks are gone: every id
        // plays the same file, so its tracks cannot tell the two apart. A
        // Stop changes nothing about the subtitles chosen.
        cmds::stop().await?;
        for _ in 0..50 {
            if cmds::player_options().await?.subtitles.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        cmds::play_episode(id, None, None).await?;
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
        let o = cmds::player_options().await?;
        if o.subtitles.len() < 2 {
            return Err(format!("episode {id} listed {} subtitle tracks, not 2", o.subtitles.len()));
        }
        Ok(o)
    }
    /// The language of the track chosen.
    fn chosen(o: &PlayerOptionsDto) -> Option<String> {
        o.subtitles.iter().find(|t| t.selected).map(|t| t.lang.clone().unwrap_or_else(|| "?".into()))
    }
    /// The language on screen: chosen and visible.
    fn shown(o: &PlayerOptionsDto) -> Option<String> {
        chosen(o).filter(|_| o.subtitles_visible)
    }
    /// Whether the title is still playing a few seconds on, its place
    /// moving: what a change of subtitles must not end.
    async fn plays_on(what: &str) -> Result<(), String> {
        let at = |s: &PlayerStateDto| s.on_demand.map_or(-1.0, |o| o.position);
        let before = cmds::player_state().await?;
        tokio::time::sleep(Duration::from_secs(4)).await;
        let after = cmds::player_state().await?;
        if after.state != PlaybackState::Playing || after.ending.is_some() || at(&after) < at(&before) + 2.0 {
            return Err(format!(
                "after {what}, the title was {:?} at {:.1}s (from {:.1}s): {}",
                after.state,
                at(&after),
                at(&before),
                after.error.as_deref().unwrap_or("no reason given")
            ));
        }
        Ok(())
    }

    // Off, as a fresh install has them: none shown, though a track is
    // chosen (hidden), so turning them on is never a switch.
    let first = open(100_111).await?;
    if let Some(lang) = shown(&first) {
        return Err(format!("subtitles were off, but the first title showed {lang}"));
    }
    let hidden = first
        .subtitles
        .iter()
        .find(|t| t.selected)
        .ok_or("subtitles were off, and no track was chosen to show later")?
        .id;

    // Mid-play, the toggle: on (the track chosen, shown), then off.
    cmds::set_subtitle_track(Some(hidden)).await?;
    plays_on("turning subtitles on").await?;
    let on = cmds::player_options().await?;
    if shown(&on).is_none() {
        return Err("turning subtitles on did not show them".into());
    }
    cmds::set_subtitle_track(None).await?;
    plays_on("turning subtitles off").await?;
    let off = cmds::player_options().await?;
    if shown(&off).is_some() || chosen(&off).is_none() {
        return Err(format!("turning subtitles off left {:?} shown, {:?} chosen", shown(&off), chosen(&off)));
    }
    cmds::set_subtitle_track(Some(hidden)).await?;
    plays_on("turning subtitles on again").await?;
    if shown(&cmds::player_options().await?).is_none() {
        return Err("turning subtitles on again did not show them".into());
    }

    // Mid-play, the menu: another track, English to French, plays on and
    // shows French. The next title shows French too.
    let french = first
        .subtitles
        .iter()
        .find(|t| t.lang.as_deref() == Some("fra"))
        .ok_or("the fixture has no French subtitles")?
        .id;
    if chosen(&cmds::player_options().await?).as_deref() == Some("fra") {
        return Err("the switch to French had nothing to switch from".into());
    }
    // Past the server's keep-alive timeout (`fake_stream.py
    // --keep-alive-timeout 5`), so the connection the episode arrived on is
    // closed when the switch goes back for the new track; and the server
    // says it closed it.
    tokio::time::sleep(IDLE_BEFORE_SWITCH).await;
    let port = fixture_port()?;
    match keep_alive(port) {
        Some((timeout, closed)) if timeout > 0.0 && timeout < IDLE_BEFORE_SWITCH.as_secs_f64() && closed == 0 => {
            return Err(format!("the server kept the idle connection past its {timeout}s timeout"));
        }
        Some(_) => {}
        None => return Err("the fixture server did not say what it does with idle connections".into()),
    }
    cmds::set_subtitle_track(Some(french)).await?;
    plays_on("switching from English to French").await?;
    let switched = cmds::player_options().await?;
    if shown(&switched).as_deref() != Some("fra") {
        return Err(format!("switched to French, the title showed {:?}", shown(&switched)));
    }
    let second = open(100_112).await?;
    if shown(&second).as_deref() != Some("fra") {
        return Err(format!("French was chosen, but the next title showed {:?}", shown(&second)));
    }

    // Off, chosen by hand: the next stays off, with French chosen, hidden.
    cmds::set_subtitle_track(None).await?;
    let third = open(100_113).await?;
    if let Some(lang) = shown(&third) {
        return Err(format!("subtitles were turned off, but the next title showed {lang}"));
    }
    if chosen(&third).as_deref() != Some("fra") {
        return Err(format!("off, the next title chose {:?}, not French hidden", chosen(&third)));
    }

    // The settings' choice, as saved settings reach the player.
    let settings = SettingsDto {
        subtitles_on: true,
        subtitle_language: Some("eng".into()),
        ..SettingsDto::default()
    };
    cmds::apply_playback_settings(settings).await?;
    let fourth = open(100_114).await?;
    if shown(&fourth).as_deref() != Some("eng") {
        return Err(format!("the settings said English, but the title showed {:?}", shown(&fourth)));
    }

    cmds::stop().await?;
    Ok("off by default (a track chosen, hidden) though a track is flagged default; mid-play on, \
        off and on again played on; a mid-play switch from English to French played on and showed \
        French, as did the next title; off chosen stayed off on the next, French hidden; the \
        settings' English showed English"
        .into())
}

/// Movie 1's TMDB id in `--find-subtitles`: the stand-in's "Film 0001".
#[cfg(target_os = "linux")]
const FILM_TMDB: i64 = 900_001;

/// Lists movie 1 in the scratch catalogue as "Film 0001" (2021), under
/// TMDB id `tmdb`.
#[cfg(target_os = "linux")]
fn seed_film(tmdb: i64) -> Result<(), String> {
    use zapper_lib::catalog::{db, vod_sync};
    let film = vod_sync::NewMovie {
        movie_id: 1,
        name: "Film 0001".into(),
        year: Some(2021),
        tmdb_id: Some(tmdb),
        container: Some("mkv".into()),
        ..vod_sync::NewMovie::default()
    };
    let now = zapper_lib::catalog::activity::now_secs();
    db::with_write_db(|c| vod_sync::land_movies(c, &[], &[film], now))
        .map(|_| ())
        .map_err(|e| format!("could not list the film in the scratch catalogue: {e}"))
}

/// The `--find-subtitles --search-only` run: a search alone, which never
/// signs in or downloads, whichever API it is pointed at.
#[cfg(target_os = "linux")]
async fn run_subtitle_search(tmdb: i64) -> Result<String, String> {
    use zapper_lib::ipc::online_cmds as online;
    use zapper_lib::ipc::types::{OnDemandKind, SubtitleSearchDto};

    seed_film(tmdb)?;
    let found = online::find_subtitles(OnDemandKind::Movie, 1, vec!["en".into()]).await?;
    Ok(match found {
        SubtitleSearchDto::Results { results, .. } => format!("results {}", results.len()),
        SubtitleSearchDto::Off => "off 0".into(),
        SubtitleSearchDto::UnknownTitle => "unknownTitle 0".into(),
        SubtitleSearchDto::Busy { .. } => "busy 0".into(),
    })
}

/// A JSON answer from a test server on this machine, over a plain socket.
#[cfg(target_os = "linux")]
fn local_json(port: u16, path: &str) -> Option<serde_json::Value> {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok()?;
    s.write_all(format!("GET {path} HTTP/1.0\r\nHost: x\r\n\r\n").as_bytes()).ok()?;
    let mut body = String::new();
    s.read_to_string(&mut body).ok()?;
    serde_json::from_str(&body[body.find("\r\n\r\n")? + 4..]).ok()
}

/// What mpv says now of the subtitle track chosen, when it is an external
/// file: its title; whether subtitles are shown; and the line drawn.
#[cfg(target_os = "linux")]
async fn external_subtitle() -> Result<(Option<String>, bool, Option<String>), String> {
    let p = zapper_lib::player::backend().ok_or("no player installed")?;
    let count = p.get_i64("track-list/count").unwrap_or(0);
    let external = (0..count).find_map(|i| {
        let key = |k: &str| format!("track-list/{i}/{k}");
        (p.get_string(&key("type")).as_deref() == Some("sub")
            && p.get_bool(&key("external")) == Some(true)
            && p.get_bool(&key("selected")) == Some(true))
        .then(|| p.get_string(&key("title")).unwrap_or_default())
    });
    Ok((external, p.get_bool("sub-visibility") == Some(true), p.get_string("sub-text")))
}

/// The `--find-subtitles` run; see the top of the file.
#[cfg(target_os = "linux")]
async fn run_find_subtitles() -> Result<String, String> {
    use std::time::{Duration, Instant};
    use zapper_lib::ipc::online_cmds as online;
    use zapper_lib::player::api as cmds;
    use zapper_lib::ipc::types::{
        OnDemandKind, PlaybackState, PlayerStateDto, SubtitleDownloadDto, SubtitleSearchDto,
    };

    const RELEASE: &str = "Film.0001.2021.1080p.TEST";
    const FILE: i64 = 7_000_001;

    let os_port: u16 = std::env::var(zapper_lib::online::opensubtitles::API_OVERRIDE)
        .ok()
        .and_then(|u| u.strip_prefix("http://127.0.0.1:")?.split('/').next()?.parse().ok())
        .ok_or("ZAPPER_OPENSUBTITLES_URL is not the stand-in on this machine")?;
    let os_stat = |key: &str| local_json(os_port, "/_stats").and_then(|v| v.get(key)?.as_i64());
    let stream_port = fixture_port()?;
    let opens = || {
        local_json(stream_port, "/vodopens")
            .and_then(|v| v.get("opens")?.as_i64())
            .ok_or("the fixture server did not say how often the film was opened")
    };
    let at = |s: &PlayerStateDto| s.on_demand.map_or(-1.0, |o| o.position);
    let playing = |s: &PlayerStateDto| {
        s.state == PlaybackState::Playing
            && s.on_demand.is_some_and(|o| o.kind == OnDemandKind::Movie && o.id == 1)
    };

    seed_film(FILM_TMDB)?;
    cmds::play_movie(1, None, None).await?;
    let opened = await_state(Duration::from_secs(25), playing).await;
    if !playing(&opened) {
        return Err(format!(
            "play_movie(1) ended in {:?}: {}",
            opened.state,
            opened.error.as_deref().unwrap_or("no reason given")
        ));
    }

    // Found: the stand-in's two English files; its third, machine
    // translated, is ranked out.
    let found = online::find_subtitles(OnDemandKind::Movie, 1, vec!["en".into()]).await?;
    let SubtitleSearchDto::Results { results, .. } = found else {
        return Err(format!("the search answered {found:?}"));
    };
    if results.len() != 2 || results[0].file_id != FILE {
        let ids: Vec<i64> = results.iter().map(|r| r.file_id).collect();
        return Err(format!("the search found {ids:?}, not {FILE} and one more"));
    }

    // Not signed in: asked to.
    let first = online::download_subtitle(OnDemandKind::Movie, 1, FILE).await?;
    if !matches!(first, SubtitleDownloadDto::SignIn { .. }) {
        return Err(format!("a download before signing in answered {first:?}"));
    }
    online::opensubtitles_sign_in("tester".into(), "test-pass".into()).await?;
    let got = online::download_subtitle(OnDemandKind::Movie, 1, FILE).await?;
    let SubtitleDownloadDto::Downloaded { subtitle, .. } = got else {
        return Err(format!("a download once signed in answered {got:?}"));
    };
    if subtitle.release != RELEASE {
        return Err(format!("the file kept is {:?}, not {RELEASE}", subtitle.release));
    }

    // Added mid-play: a few seconds in, inside the file's lines (1s to 41s).
    let before = await_state(Duration::from_secs(15), |s| playing(s) && at(s) >= 5.0).await;
    if !playing(&before) || at(&before) < 5.0 {
        return Err(format!(
            "the film was {:?} at {:.1}s before the file was added",
            before.state,
            at(&before)
        ));
    }
    let opens_before = opens()?;
    if opens_before != 1 {
        return Err(format!("the film was opened {opens_before} times before the file was added"));
    }
    let title = format!("Downloaded · {RELEASE}");
    let added =
        online::add_subtitle_file(OnDemandKind::Movie, 1, FILE, title.clone(), "en".into())
            .await?;
    if !added.subtitles_visible {
        return Err("adding the file left subtitles hidden".into());
    }
    let added_at = Instant::now();

    let mut now = external_subtitle().await?;
    while now.0.is_none() && added_at.elapsed() < Duration::from_secs(3) {
        tokio::time::sleep(Duration::from_millis(100)).await;
        now = external_subtitle().await?;
    }
    match &now.0 {
        None => return Err("3s after the file was added, mpv had no external track chosen".into()),
        Some(t) if *t != title => return Err(format!("the track added is named {t:?}, not {title:?}")),
        Some(_) => {}
    }
    if !now.1 {
        return Err("the file was added with subtitles hidden".into());
    }
    while now.2.is_none() && added_at.elapsed() < Duration::from_secs(4) {
        tokio::time::sleep(Duration::from_millis(100)).await;
        now = external_subtitle().await?;
    }
    let line = now.2.clone().ok_or_else(|| {
        format!("4s after the file was added (at {:.1}s), mpv drew no line of it", at(&before))
    })?;
    if !line.contains(&format!("of file {FILE}")) {
        return Err(format!("mpv drew {line:?}, not a line of the file added"));
    }

    // The film plays on, without being opened again.
    tokio::time::sleep(Duration::from_secs(4)).await;
    let after = cmds::player_state().await?;
    if !playing(&after) || after.ending.is_some() || at(&after) < at(&before) + 3.0 {
        return Err(format!(
            "after the file was added, the film was {:?} at {:.1}s (from {:.1}s): {}",
            after.state,
            at(&after),
            at(&before),
            after.error.as_deref().unwrap_or("no reason given")
        ));
    }
    let opens_after = opens()?;
    if opens_after != 1 {
        return Err(format!("adding the file opened the film again ({opens_after} opens)"));
    }
    // The picture menu hears of it too, as the track chosen.
    let listed = cmds::player_options().await?;
    if !listed.subtitles.iter().any(|t| t.selected && t.title.as_deref() == Some(title.as_str())) {
        return Err("the picture menu does not list the file added as chosen".into());
    }

    // Kept: a second download asks the stand-in nothing, and no token ever
    // went to where the file is fetched from.
    let again = online::download_subtitle(OnDemandKind::Movie, 1, FILE).await?;
    if !matches!(again, SubtitleDownloadDto::Downloaded { .. }) {
        return Err(format!("a second download answered {again:?}"));
    }
    match (os_stat("downloads"), os_stat("authorized_files")) {
        (Some(1), Some(0)) => {}
        (d, a) => {
            return Err(format!("the stand-in counted {d:?} downloads and {a:?} files fetched with a token"))
        }
    }

    // Stopped, there is nothing to add the file to.
    cmds::stop().await?;
    match online::add_subtitle_file(OnDemandKind::Movie, 1, FILE, title, "en".into()).await {
        Err(e) if e == "That title is no longer playing." => {}
        other => return Err(format!("adding the file once stopped answered {other:?}")),
    }
    Ok(format!(
        "ok: 2 English files found; asked to sign in, signed in, downloaded; added at {:.1}s, \
         chosen and shown, drawing {line:?}; the film played on to {:.1}s, opened once; a \
         second download asked nothing",
        at(&before),
        at(&after)
    ))
}

/// Saves what mpv's window shows to `path`: the picture as mpv drew it into
/// the window, bands, subtitles and all (`screenshot-to-file … window`).
#[cfg(target_os = "linux")]
async fn grab(path: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let p = zapper_lib::player::backend().ok_or("no window to picture")?;
        p.command("screenshot-to-file", &[&path, "window"])
            .map_err(|e| format!("picturing the window: {e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// How long `--subtitles` lets the episode's connection sit idle before the
/// mid-play switch: past `fake_stream.py --keep-alive-timeout 5`.
#[cfg(target_os = "linux")]
const IDLE_BEFORE_SWITCH: std::time::Duration = std::time::Duration::from_secs(8);

/// The fixture server's keep-alive timeout (0: none) and how many idle
/// connections it has closed, from its /keepalive.
#[cfg(target_os = "linux")]
fn keep_alive(port: u16) -> Option<(f64, i64)> {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok()?;
    s.write_all(b"GET /keepalive HTTP/1.0\r\nHost: x\r\n\r\n").ok()?;
    let mut body = String::new();
    s.read_to_string(&mut body).ok()?;
    let v: serde_json::Value = serde_json::from_str(&body[body.find("\r\n\r\n")? + 4..]).ok()?;
    Some((v.get("timeout")?.as_f64()?, v.get("idleClosed")?.as_i64()?))
}

/// The `--captions` run; see the top of the file.
#[cfg(target_os = "linux")]
async fn run_captions() -> Result<String, String> {
    use std::time::{Duration, Instant};
    use zapper_lib::player::api as cmds;
    use zapper_lib::ipc::types::PlaybackState;

    /// What mpv says: the caption text and the
    /// placement properties, then the window and the picture's bands in it
    /// (`osd-dimensions`).
    const KEYS: [&str; 9] = [
        "sub-text/ass",
        "sub-ass-override",
        "sub-pos",
        "sub-align-x",
        "sub-margin-x",
        "osd-dimensions/w",
        "osd-dimensions/ml",
        "osd-dimensions/mr",
        "sub-visibility",
    ];
    async fn read() -> Option<[String; 9]> {
        let p = zapper_lib::player::backend()?;
        Some(KEYS.map(|k| p.get_string(k).unwrap_or_default()))
    }
    let num = |s: &str| s.parse::<f64>().unwrap_or(f64::NAN);

    // mpv's own values, before anything is played.
    let before = read().await.ok_or("could not read mpv's properties")?;
    let id = resolve_stream_id().await?;
    cmds::play(id).await?;
    let opened = await_state(Duration::from_secs(25), |s| s.state == PlaybackState::Playing).await;
    if opened.state != PlaybackState::Playing {
        return Err(format!("the captioned channel ended in {:?}", opened.state));
    }
    // mpv lists the captions once it has met some in the picture.
    let mut caption_track = None;
    for _ in 0..100 {
        let o = cmds::player_options().await?;
        caption_track = o.subtitles.iter().find(|t| t.codec.as_deref() == Some("eia_608")).map(|t| t.id);
        if caption_track.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let track = caption_track.ok_or("no caption track was listed")?;
    cmds::set_subtitle_track(Some(track)).await?;

    // The fixture loops three captions: top left, centred at the bottom, and
    // on the right. Each one's placement, once mpv draws it.
    let shots = std::env::var("ZAPPER_CAPTION_SHOTS").ok();
    let name = std::env::var("ZAPPER_FIXTURE_NAME").unwrap_or_default();
    let tag = name.trim_end_matches(".ts");
    let want = [("TOP LEFT", "left", "top-left"), ("THE BOTTOM", "center", "bottom-centre"), ("RIGHT SIDE", "right", "right")];
    let mut seen = Vec::new();
    // Every distinct reading, for a run that fails to say what mpv did.
    let mut readings: Vec<[String; 9]> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(45);
    for (text, align, what) in want {
        loop {
            if Instant::now() > deadline {
                return Err(format!(
                    "the {what} caption was not seen placed (seen: {seen:?}; mpv said {readings:?})"
                ));
            }
            let Some(v) = read().await else {
                return Err("could not read mpv's properties".into());
            };
            if readings.last() != Some(&v) && readings.len() < 40 {
                readings.push(v.clone());
            }
            // One caption on screen, once: the same event twice is a caption
            // drawn twice, which must not pass for a placed one.
            let events: Vec<&str> = v[0].split('\n').filter(|e| !e.is_empty()).collect();
            if events.iter().enumerate().any(|(i, e)| events[..i].contains(e)) {
                return Err(format!("a caption was drawn twice: {:?}", v[0]));
            }
            if !v[0].contains(text) {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            // The placement follows the caption on the next heartbeat.
            tokio::time::sleep(Duration::from_millis(600)).await;
            let Some(v) = read().await else {
                return Err("could not read mpv's properties".into());
            };
            if !v[0].contains(text) {
                continue;
            }
            if v[3] != align {
                return Err(format!("the {what} caption was aligned {}, not {align}", v[3]));
            }
            if v[1] != "strip" {
                return Err(format!("the {what} caption was drawn with sub-ass-override={}", v[1]));
            }
            let (pos, margin, w, ml, mr) = (num(&v[2]), num(&v[4]), num(&v[5]), num(&v[6]), num(&v[7]));
            // Where mpv puts a side caption's edge, in the window's pixels
            // (see player::captions): inside the picture, never in a band.
            let edge = margin * (w - ml - mr) / 960.0;
            match what {
                "top-left" if !(pos < 40.0 && edge > ml + 1.0) => {
                    return Err(format!("top left placed at sub-pos {pos}, its edge {edge:.0}px with a {ml}px band"));
                }
                "bottom-centre" if pos < 70.0 => {
                    return Err(format!("the bottom caption placed at sub-pos {pos}"));
                }
                "right" if !(pos > 70.0 && edge > mr + 1.0) => {
                    return Err(format!("the right caption placed at sub-pos {pos}, its edge {edge:.0}px with a {mr}px band"));
                }
                _ => {}
            }
            // A moment for the frame to carry it, then the picture.
            tokio::time::sleep(Duration::from_millis(300)).await;
            if let Some(dir) = &shots {
                grab(format!("{dir}/{tag}-{what}.png")).await?;
            }
            seen.push(format!("{what}: sub-pos {pos} {align} margin {margin} (edge {edge:.0}px, bands {ml}/{mr}px of {w})"));
            break;
        }
    }

    // Off: hidden, the caption track kept, and the channel plays on.
    cmds::set_subtitle_track(None).await?;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let o = cmds::player_options().await?;
    if o.subtitles_visible || !o.subtitles.iter().any(|t| t.selected && t.id == track) {
        return Err("captions turned off were not hidden with their track kept".into());
    }
    if cmds::player_state().await?.state != PlaybackState::Playing {
        return Err("turning captions off stopped the channel".into());
    }
    // No captions track any more (stopped): mpv's own values back.
    cmds::stop().await?;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let v = read().await.ok_or("could not read mpv's properties")?;
    // The placement only: how a channel's subtitles are styled
    // (sub-ass-override) is the open's, set again by the next one.
    if v[2..5] != before[2..5] {
        return Err(format!("with no captions track, mpv was left with {:?}, not {:?}", &v[2..5], &before[2..5]));
    }
    Ok(format!(
        "{}; turned off, hidden with the track kept, playing on; with no captions track, mpv's \
         own placement ({}) again",
        seen.join("; "),
        before[2..5].join(", ")
    ))
}

/// The `--up-next` run; see the top of the file.
#[cfg(target_os = "linux")]
async fn run_up_next(drop: bool, past_end: bool) -> Result<String, String> {
    use std::time::Duration;
    use zapper_lib::player::api as cmds;
    use zapper_lib::catalog::vod_store::ItemKind;
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

    cmds::play_episode(A, None, None).await?;
    let opened = await_state(Duration::from_secs(25), playing(A)).await;
    if !playing(A)(&opened) {
        return Err(failure("play_episode(A)", &opened));
    }

    if drop {
        // Past what the server sends before it cuts the file short.
        cmds::seek_to(90.0).await?;
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
        if cmds::play_next_episode(A, B).await.is_ok() {
            return Err("moving on from an episode cut short was not refused".into());
        }
        let after = cmds::player_state().await?;
        if after.state != PlaybackState::Failed || episode(after.on_demand) != Some(A) {
            return Err(format!("a refused move on left the player in {:?}", after.state));
        }
        cmds::stop().await?;
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
        cmds::seek_to(85.0).await?;
        let there = await_state(Duration::from_secs(8), |s| (84.0..=90.0).contains(&at(s))).await;
        if !(84.0..=90.0).contains(&at(&there)) {
            return Err(format!("seek_to(85) left A at {:.1}s", at(&there)));
        }
        cmds::seek_relative(60.0).await?;
    } else {
        cmds::seek_to(115.0).await?;
    }
    let ended = await_state(Duration::from_secs(30), |s| s.state != PlaybackState::Playing).await;
    if ended.state != PlaybackState::Stopped {
        return Err(failure("an episode played to its end", &ended));
    }
    if episode(ended.finished) != Some(A) {
        return Err(format!("an episode played to its end reported {:?} as finished", ended.finished));
    }

    // Moving on from it.
    cmds::play_next_episode(A, B).await?;
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
    cmds::set_subtitle_track(Some(1)).await?;
    cmds::seek_to(105.0).await?;
    let near = await_state(Duration::from_secs(8), |s| at(s) >= 104.0).await;
    if at(&near) < 104.0 || near.state != PlaybackState::Playing {
        return Err(format!("seek_to(105) left B at {:.1}s, {:?}", at(&near), near.state));
    }
    // C was left part way: it opens there, without asking.
    zapper_lib::catalog::db::with_db(|c| {
        let at = zapper_lib::catalog::activity::now_secs();
        zapper_lib::catalog::vod_store::record_progress(c, ItemKind::Episode, C, 60.0, Some(120.0), at)
    })
    .map_err(|e| format!("writing C's place in the scratch catalogue: {e}"))?;
    // At once, as a countdown running out under a press would.
    let (first, second) = tokio::join!(
        cmds::play_next_episode(B, C),
        cmds::play_next_episode(B, C)
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
    if !(58.0..=62.0).contains(&at(&last)) {
        return Err(format!("C, left at 60s, opened at {:.1}s", at(&last)));
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
    cmds::toggle_pause().await?;
    let paused = await_state(Duration::from_secs(5), |s| s.state == PlaybackState::Paused).await;
    if paused.state != PlaybackState::Paused || episode(paused.on_demand) != Some(C) {
        return Err(format!("toggle_pause() left C in {:?}", paused.state));
    }
    cmds::play_next_episode(C, D).await?;
    let resumed = await_state(Duration::from_secs(25), playing(D)).await;
    if !playing(D)(&resumed) {
        return Err(failure("play_next_episode(C, D) from a paused C", &resumed));
    }

    cmds::stop().await?;
    let most = streams.stop()?;
    if most >= 2 {
        return Err(format!("a second stream lingered on the fixture server ({most} open)"));
    }
    Ok(format!(
        "A {} as finished; A to B opened B and marked A watched; B to C at {:.0}s \
         opened C once at {:.0}s where it was left, marked B watched and kept the subtitles; \
         a paused C to D opened D playing; no second stream lingered",
        if past_end { "skipped past its end" } else { "played to its end" },
        at(&near),
        at(&last)
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
async fn note(stage: &str) {
    let d = zapper_lib::surface::mpv::created()
        .map(|p| p.diagnostics())
        .unwrap_or_else(|| "no player in this process".to_owned());
    println!("[{stage}] {d}");
    if let Ok(s) = zapper_lib::player::api::player_state().await {
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
        let s = zapper_lib::player::api::player_state()
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
    let channels = zapper_lib::player::controller::xtream()?
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

/// Standard base64 back to bytes (padding and anything else ignored).
#[cfg(target_os = "linux")]
fn decode_base64(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0);
    for v in s.bytes().filter_map(val) {
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// `--trickplay-vod`; see the module's comment.
#[cfg(target_os = "linux")]
async fn run_trickplay_vod() -> Result<String, String> {
    use std::time::{Duration, Instant};
    use zapper_lib::player::api as cmds;
    use zapper_lib::ipc::types::PlaybackState;
    use zapper_lib::surface::{mpv as zmpv, title_thumbs, trickplay};

    let mpv = zmpv::handle().ok_or("no mpv")?;
    let ranges = || -> Vec<(f64, f64)> {
        let raw = mpv
            .get_property::<String>("demuxer-cache-state")
            .unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
        v.get("seekable-ranges")
            .and_then(|r| r.as_array())
            .map(|rs| {
                rs.iter()
                    .filter_map(|r| Some((r.get("start")?.as_f64()?, r.get("end")?.as_f64()?)))
                    .collect()
            })
            .unwrap_or_default()
    };
    let pos = || mpv.get_property::<f64>("time-pos").unwrap_or(-1.0);
    let spacing = trickplay::TITLE_SPACING_S;
    let at = |k: i64| k as f64 * spacing;
    let mut notes = Vec::new();

    // ----- coverage, with a buffer of about half a minute
    // (`ZAPPER_TRICKPLAY_CACHE_MB`, 6 for the 120 s film; more for a film of
    // a higher bitrate), and `ZAPPER_TRICKPLAY_FILL_SECS` (4) for the worker
    // to fill it, longer where excerpts are big and the worker paces itself.
    let env_num = |k: &str, d: u64| std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d);
    let fill_secs = env_num("ZAPPER_TRICKPLAY_FILL_SECS", 4);
    mpv.set_property("demuxer-max-bytes", env_num("ZAPPER_TRICKPLAY_CACHE_MB", 6) as i64 * 1024 * 1024)
        .map_err(|e| format!("demuxer-max-bytes: {e}"))?;
    cmds::play_movie(1, None, None).await?;
    let opened = await_state(Duration::from_secs(25), |s| s.state == PlaybackState::Playing).await;
    if opened.state != PlaybackState::Playing {
        return Err(format!("play_movie() ended in {:?}", opened.state));
    }
    let length = opened.on_demand.and_then(|o| o.duration).unwrap_or(120.0);
    tokio::time::sleep(Duration::from_secs(10)).await;
    // Paused, so the buffer and the playhead hold still while they are read.
    cmds::set_paused(true).await?;
    tokio::time::sleep(Duration::from_secs(fill_secs)).await;
    let (head, rs, times) = (pos(), ranges(), trickplay::title_times());
    let end = rs
        .iter()
        .find(|r| r.0 <= head && head <= r.1)
        .map(|r| r.1)
        .ok_or_else(|| format!("no buffered range holds the playhead {head:.1}: {rs:?}"))?;
    let stride = title_thumbs::stats().stride.max(1);
    println!("[trickplay] playhead {head:.1}s, buffered {rs:?}, every {stride} slot(s), frames at {times:?}");
    if end > length - 15.0 {
        return Err(format!("the buffer was meant to be short, but it reaches {end:.1}s of {length:.0}"));
    }
    let first = (head / spacing).ceil() as i64;
    let last = ((end - trickplay::DUMP_SPAN_S) / spacing).floor() as i64;
    if last < first + 1 {
        return Err(format!("too little buffered ahead to check: {head:.1}..{end:.1}"));
    }
    // Every slot the worker tries (all of them, unless big excerpts made
    // it try every second or fourth) has a frame.
    let tried = |k: &i64| k % stride == 0;
    if let Some(k) = (first..=last).filter(tried).find(|k| trickplay::title_frame(at(*k)).is_none()) {
        return Err(format!("slot {k} ({}s) is buffered and has no frame; frames at {times:?}", at(k)));
    }
    let past_end = |t: f64| at(trickplay::slot_of(t, spacing)) > end + 0.01;
    if let Some(past) = times.iter().find(|t| past_end(**t)) {
        return Err(format!("a frame at {past:.1}s, past the buffered end {end:.1}s"));
    }
    let answer = cmds::thumbnail_for_title(at(first)).await?;
    if !answer
        .as_ref()
        .is_some_and(|t| t.data_url.starts_with("data:image/jpeg;base64,/9j/"))
    {
        let head: Option<String> = answer.map(|t| t.data_url.chars().take(30).collect());
        return Err(format!("thumbnail_for_title gave {head:?}"));
    }
    if let (Ok(dir), Some(t)) = (std::env::var("ZAPPER_TRICKPLAY_SAVE"), &answer) {
        // The preview as the seek bar receives it, to look at.
        let b64 = t.data_url.split_once(',').map_or("", |(_, b)| b);
        let path = std::path::Path::new(&dir).join(format!("slot-{first}.jpg"));
        std::fs::write(&path, decode_base64(b64)).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let never = length - 10.0;
    if cmds::thumbnail_for_title(never).await?.is_some() {
        return Err(format!("thumbnail_for_title answered for {never:.0}s, which was never buffered"));
    }
    notes.push(format!(
        "slots {first}..={last} filled up to the buffered end {end:.1}s, none past it"
    ));

    // A seek ahead, past the buffer: the new stretch fills too.
    cmds::seek_to(80.0).await?;
    cmds::set_paused(false).await?;
    tokio::time::sleep(Duration::from_secs(5)).await;
    cmds::set_paused(true).await?;
    tokio::time::sleep(Duration::from_secs(fill_secs)).await;
    let (head2, rs2) = (pos(), ranges());
    let stride = title_thumbs::stats().stride.max(1);
    let end2 = rs2
        .iter()
        .find(|r| r.0 <= head2 && head2 <= r.1)
        .map_or(head2, |r| r.1);
    let ahead: Vec<i64> = ((head2 / spacing).ceil() as i64
        ..=((end2 - trickplay::DUMP_SPAN_S) / spacing).floor() as i64)
        .filter(|k| k % stride == 0)
        .collect();
    if let Some(k) = ahead.iter().find(|k| trickplay::title_frame(at(**k)).is_none()) {
        return Err(format!(
            "after a seek to 80s, slot {k} is buffered ({head2:.1}..{end2:.1}) and has no frame: {:?}",
            trickplay::title_times()
        ));
    }
    notes.push(format!("after a seek to 80s, slots {ahead:?} filled"));

    // The same title opened again keeps them.
    let before = trickplay::title_times().len();
    cmds::play_movie(1, None, None).await?;
    let kept = trickplay::title_times().len();
    if kept < before {
        return Err(format!("opening the same film again dropped its frames: {before} -> {kept}"));
    }
    notes.push(format!("{kept} frames kept across a reopen"));

    // ----- playback with and without the worker, the whole film buffered
    mpv.set_property("demuxer-max-bytes", zmpv::DEMUXER_MAX_BYTES)
        .map_err(|e| format!("demuxer-max-bytes: {e}"))?;
    // `ZAPPER_TRICKPLAY_HWDEC=no` decodes the film on the processor too, the
    // harder case for a worker that decodes beside it.
    if let Ok(hwdec) = std::env::var("ZAPPER_TRICKPLAY_HWDEC") {
        mpv.set_property("hwdec", hwdec.as_str())
            .map_err(|e| format!("hwdec: {e}"))?;
    }
    cmds::play_movie(1, Some(0.0), None).await?;
    await_state(Duration::from_secs(25), |s| s.state == PlaybackState::Playing).await;
    cmds::set_paused(false).await?;
    tokio::time::sleep(Duration::from_secs(6)).await;
    let secs: u64 = std::env::var("ZAPPER_TRICKPLAY_MEASURE_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let count = |k: &str| mpv.get_property::<i64>(k).unwrap_or(-1);
    let mut lines = Vec::new();
    let mut idle_worker = false;
    for (i, on) in [false, true, false].into_iter().enumerate() {
        title_thumbs::set_enabled(on);
        // From near the start each time, so every phase plays the same pictures.
        cmds::seek_to(2.0).await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let (vo0, dec0) = (count("frame-drop-count"), count("decoder-frame-drop-count"));
        let s0 = title_thumbs::stats();
        let began = Instant::now();
        let mut samples: Vec<(f64, f64)> = Vec::new();
        let mut stressed = Instant::now();
        while began.elapsed() < Duration::from_secs(secs) {
            if on && stressed.elapsed() >= Duration::from_secs(2) {
                // A new, empty store: every buffered slot is to fill again.
                trickplay::open_title(&format!("stress-{}", began.elapsed().as_millis()));
                stressed = Instant::now();
            }
            samples.push((began.elapsed().as_secs_f64(), pos()));
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let vo = count("frame-drop-count") - vo0;
        let dec = count("decoder-frame-drop-count") - dec0;
        let s1 = title_thumbs::stats();
        let slots = (s1.filed + s1.failed + s1.repeats) - (s0.filed + s0.failed + s0.repeats);
        let per = |v: u64| if slots == 0 { 0.0 } else { v as f64 / slots as f64 };
        let (mut still, mut worst, mut moved) = (0.0f64, 0.0f64, samples[0]);
        for w in samples.windows(2) {
            let (dw, dp) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
            worst = worst.max((dp - dw).abs());
            if dp > 0.0 {
                moved = w[1];
            } else {
                still = still.max(w[1].0 - moved.0);
            }
        }
        let (t0, p0) = samples[0];
        let (t1, p1) = samples[samples.len() - 1];
        let drift = (p1 - p0) - (t1 - t0);
        let decoder = mpv
            .get_property::<String>("hwdec-current")
            .ok()
            .filter(|s| !s.is_empty() && s != "no")
            .unwrap_or_else(|| "software".into());
        let line = format!(
            "phase {} worker {} ({decoder} decoding): {secs}s, dropped vo={vo} decoder={dec}, \
             time-pos drift {drift:+.3}s, longest still {:.0}ms, worst 50ms sample off by {:.0}ms, \
             slots {slots} (filed {}, no picture {}, repeat keyframe {}), {:.1}MB written \
             ({:.2}MB a slot, largest {:.1}MB), {:.1}ms a slot (longest {:.1}ms), \
             worker CPU {:.0}ms = {:.1}% of a core",
            i + 1,
            if on { "on" } else { "off" },
            still * 1000.0,
            worst * 1000.0,
            s1.filed - s0.filed,
            s1.failed - s0.failed,
            s1.repeats - s0.repeats,
            (s1.bytes - s0.bytes) as f64 / 1048576.0,
            per(s1.bytes - s0.bytes) / 1048576.0,
            s1.largest as f64 / 1048576.0,
            per(s1.busy_us - s0.busy_us) / 1000.0,
            s1.longest_us as f64 / 1000.0,
            (s1.cpu_us - s0.cpu_us) as f64 / 1000.0,
            (s1.cpu_us - s0.cpu_us) as f64 / 1000.0 / secs as f64 / 10.0,
        );
        println!("[trickplay] {line}");
        lines.push(line);
        idle_worker |= on && slots == 0;
    }
    title_thumbs::set_enabled(true);
    cmds::stop().await?;
    tokio::time::sleep(Duration::from_millis(600)).await;
    // The excerpts' folder, in this run's scratch cache.
    let excerpts = zapper_lib::paths::cache_dir().join("previews");
    let leftovers: Vec<String> = std::fs::read_dir(&excerpts)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    if !leftovers.is_empty() {
        return Err(format!("excerpts left behind: {leftovers:?}"));
    }
    if idle_worker {
        return Err(format!("the worker decoded nothing while it was on: {}", lines.join("; ")));
    }
    Ok(format!("{}; {}", notes.join("; "), lines.join("; ")))
}
