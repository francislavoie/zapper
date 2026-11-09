//! The headless host (`surface::host`) against a real stream: mpv in a
//! window of its own, driven through the installed backend directly, with no
//! controller and no Tauri.
//!
//! Opens the host muted, loads a film from the fixture server and waits for
//! it to play in mpv's own window (`gpu-next`, configured); waits for live
//! scrollback thumbnails from `screenshot-raw` (two, the clock's pace, and a
//! picture rather than a blank); then a seek storm, 50 relative seeks in two
//! seconds while another thread pumps and probes as the heartbeat does,
//! which must neither hang nor lose a seek: paused for it, so the position
//! it settles at is the start plus the seeks' sum, to within a second; then
//! it plays on.
//!
//! Usage: `tools/fake_stream.py --movies 2 &`, then
//! `cargo run --features harness --bin host_smoke` inside a desktop session
//! (the headless gnome-shell for anything unattended). The film is
//! `ZAPPER_FIXTURE_MOVIE` (default `movie/u/p/2.mp4`, 120 s) on
//! `ZAPPER_FIXTURE_BASE` (default `http://127.0.0.1:8788`), on a scratch
//! account: never the account on this machine. Audio is off outright
//! (`ao=null`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use zapper_lib::player::{self, PlayerBackend, Signal};
use zapper_lib::surface::{host, mpv, trickplay};

/// Exits without unwinding: mpv's threads, its window among them, are still
/// running, and nothing in a one-shot harness is worth cleaning up.
fn finish(code: i32) -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(code)
}

fn fail(what: impl std::fmt::Display) -> ! {
    println!("FAIL: {what}");
    finish(1)
}

/// Pumps until `done` says so (given what each pump brought), failing on a
/// playback error or after `limit`.
fn wait_for(
    p: &dyn PlayerBackend,
    what: &str,
    limit: Duration,
    mut done: impl FnMut(&dyn PlayerBackend, &[Signal]) -> bool,
) {
    let until = Instant::now() + limit;
    loop {
        let signals = p.pump();
        for s in &signals {
            if let Signal::EndFile { error: Some(e), .. } = s {
                fail(format!("{what}: playback failed ({e})"));
            }
        }
        if done(p, &signals) {
            return;
        }
        if Instant::now() > until {
            fail(format!("{what}: not within {limit:?}"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Waits for mpv to finish seeking, the clock readable again.
fn settle(p: &dyn PlayerBackend, what: &str) -> f64 {
    wait_for(p, what, Duration::from_secs(10), |p, _| {
        p.get_bool("seeking") == Some(false) && p.get_f64("time-pos").is_some()
    });
    p.get_f64("time-pos").unwrap_or(f64::NAN)
}

fn main() {
    tracing_subscriber::fmt::init();

    // A scratch account and the fixture server, before any thread exists:
    // this changes the environment.
    zapper_lib::harness::isolate("host_smoke");
    let base = std::env::var("ZAPPER_FIXTURE_BASE")
        .unwrap_or_else(|_| zapper_lib::harness::FIXTURE_BASE.to_owned());
    let movie =
        std::env::var("ZAPPER_FIXTURE_MOVIE").unwrap_or_else(|_| "movie/u/p/2.mp4".to_owned());
    let url = format!("{}/{}", base.trim_end_matches('/'), movie.trim_start_matches('/'));
    if let Err(e) = zapper_lib::harness::local_url(&url) {
        fail(format!("ZAPPER_FIXTURE_BASE: {e}"));
    }

    // A hung player must not leave a window behind.
    let limit: u64 = std::env::var("ZAPPER_CHECK_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(90);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(limit));
        fail(format!("the run did not finish within {limit}s"));
    });

    let opened = host::open_host(host::HostOptions {
        title: "zapper host_smoke".to_owned(),
        muted: true,
        window: None,
    })
    .unwrap_or_else(|e| fail(format!("opening the host: {e}")));
    let Some(p) = player::backend() else {
        fail("open_host did not install the player");
    };
    if !Arc::ptr_eq(&opened, &p) {
        fail("the installed player is not the one open_host returned");
    }
    let p = &*p;
    if p.get_string("ao").as_deref() != Some("null") {
        fail(format!("audio output is {:?}, not null", p.get_string("ao")));
    }

    // Plays, in mpv's own window.
    let began = Instant::now();
    if let Err(e) = p.load(&url) {
        fail(format!("load: {e:#}"));
    }
    let mut reconfigured = false;
    wait_for(p, "playing", Duration::from_secs(30), |p, signals| {
        reconfigured |= signals.contains(&Signal::VideoReconfig);
        reconfigured && p.get_f64("time-pos").is_some_and(|t| t > 1.0)
    });
    let vo = p.get_string("current-vo");
    if vo.as_deref() != Some("gpu-next") || p.get_bool("vo-configured") != Some(true) {
        fail(format!("the video output is {vo:?}, configured {:?}", p.get_bool("vo-configured")));
    }
    let (w, h) = (p.get_i64("osd-width"), p.get_i64("osd-height"));
    if !w.zip(h).is_some_and(|(w, h)| w > 0 && h > 0) {
        fail(format!("mpv's window has no size ({w:?}x{h:?})"));
    }
    println!(
        "playing after {:.1}s: {:?}x{:?} video in a {}x{} {} window",
        began.elapsed().as_secs_f64(),
        p.get_i64("video-params/w"),
        p.get_i64("video-params/h"),
        w.unwrap_or(0),
        h.unwrap_or(0),
        vo.unwrap_or_default(),
    );

    // Live thumbnails: the first is due at once, the next four seconds on.
    wait_for(p, "a scrollback thumbnail", Duration::from_secs(10), |_, _| {
        trickplay::count() >= 1
    });
    let first = Instant::now();
    wait_for(p, "a second scrollback thumbnail", Duration::from_secs(10), |_, _| {
        trickplay::count() >= 2
    });
    let took = first.elapsed().as_secs_f64();
    if took < 3.0 {
        fail(format!("thumbnails {took:.1}s apart: faster than the clock's pace"));
    }
    let at = p.get_f64("playback-time").unwrap_or(0.0);
    let Some((near, png)) = trickplay::nearest_png(at) else {
        fail(format!("no thumbnail near {at:.1}s"));
    };
    let Some(shot) = mpv::screenshot_thumbnail() else {
        fail("screenshot-raw gave no frame");
    };
    let (lo, hi) = shot
        .as_chunks::<4>()
        .0
        .iter()
        .map(|px| u16::from(px[0]) + u16::from(px[1]) + u16::from(px[2]))
        .fold((u16::MAX, 0), |(lo, hi), v| (lo.min(v), hi.max(v)));
    if hi.saturating_sub(lo) < 30 {
        fail(format!("the thumbnail is blank (brightness {lo}..{hi})"));
    }
    println!(
        "thumbnails: {}, {took:.1}s apart, nearest {near:.1}s to {at:.1}s ({} bytes of PNG)",
        trickplay::count(),
        png.len()
    );

    // The seek storm, with a heartbeat pumping and probing beside it. Paused,
    // so the clock moves only by the seeks and the sum is exact (hr-seek
    // lands where it is asked to): a lost or doubled seek shows.
    if let Err(e) = p.set_flag("pause", true) {
        fail(format!("pausing before the storm: {e:#}"));
    }
    let start = settle(p, "settling before the storm");
    let stop = Arc::new(AtomicBool::new(false));
    let ticker = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let mut ticks = 0u32;
            while !stop.load(Ordering::Acquire) {
                if let Some(p) = player::backend() {
                    p.pump();
                    p.probe();
                    ticks += 1;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            ticks
        })
    };
    // +2, -1, …: 25 s forward in all, from a few seconds into a 120 s film.
    let deltas: Vec<f64> = (0..50).map(|i| if i % 2 == 0 { 2.0 } else { -1.0 }).collect();
    let sum: f64 = deltas.iter().sum();
    let storm = Instant::now();
    for (i, by) in deltas.iter().enumerate() {
        if let Err(e) = p.command("seek", &[&format!("{by:.3}"), "relative"]) {
            fail(format!("seek {i}: {e:#}"));
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    let sent = storm.elapsed();
    stop.store(true, Ordering::Release);
    let ticks = ticker.join().unwrap_or_else(|_| fail("the heartbeat panicked"));
    let end = settle(p, "settling after the storm");
    let elapsed = storm.elapsed().as_secs_f64();
    let residue = end - start - sum;
    println!(
        "seek storm: 50 seeks in {:.2}s, {ticks} heartbeat ticks beside them; \
         {start:.2}s + {sum:.0}s -> {end:.2}s ({residue:+.2}s, {elapsed:.2}s elapsed)",
        sent.as_secs_f64()
    );
    if residue.abs() >= 1.0 {
        fail(format!("after the storm at {end:.2}s, not {start:.2}s + {sum:.0}s"));
    }
    if let Err(e) = p.set_flag("pause", false) {
        fail(format!("resuming after the storm: {e:#}"));
    }
    wait_for(p, "playing on after the storm", Duration::from_secs(5), |p, _| {
        p.get_f64("time-pos").is_some_and(|t| t > end + 1.0)
    });

    let _ = p.command("stop", &[]);
    println!("PASS: host_smoke");
    finish(0)
}
