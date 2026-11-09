//! What the first half-minute of a channel actually looks like.
//!
//! Switching channels shows a moment of video, stutters and jumps, plays
//! roughly for a few seconds, then stutters again. This samples the two
//! numbers that would explain it: where the playhead is, and how far it sits
//! behind the end of mpv's seekable window — which is the value the app's
//! live-follow uses to decide the viewer has drifted and needs catching up.
//!
//! The suspicion is that the value means something different during startup.
//! A live stream arrives faster than realtime while the cache fills, so the
//! seekable end runs away from a playhead that is doing nothing wrong, and
//! anything chasing it will seek repeatedly during exactly the seconds the
//! picture is trying to settle.
//!
//! This harness does **not** run the app's follow logic. That is the point:
//! if playback here is smooth and the app stutters, the stutter is ours.
//!
//! Usage:
//!   tools/fake_stream.py &
//!   cargo run -p zapper-mpv --features harness --bin open_bench -- http://127.0.0.1:8788/sample1080.ts

use std::time::{Duration, Instant};

/// The app's own heartbeat, so the numbers line up with what it would see.
const TICK: Duration = Duration::from_millis(250);
const WATCH: Duration = Duration::from_secs(25);

/// The app's threshold for "the viewer has drifted, catch them up".
const FOLLOW_SLACK: f64 = 6.0;

/// Seconds between the playhead and the end of the seekable window.
///
/// The same derivation `Player::probe` uses: the last seekable range's end,
/// falling back to the cache end, minus where playback is.
fn behind_live(mpv: &libmpv2::Mpv) -> Option<f64> {
    let pos: f64 = mpv.get_property("time-pos").ok()?;
    let raw: String = mpv.get_property("demuxer-cache-state").ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let end = v
        .get("seekable-ranges")
        .and_then(|r| r.as_array())
        .and_then(|a| a.last())
        .and_then(|r| r.get("end"))
        .and_then(serde_json::Value::as_f64)
        .or_else(|| v.get("cache-end").and_then(serde_json::Value::as_f64))?;
    Some((end - pos).max(0.0))
}

fn main() {
    // The fixture server's sample by default; a URL off this machine only
    // with ZAPPER_ALLOW_REAL_ACCOUNT=1 (see zapper_core::harness).
    let url = match zapper_core::harness::stream_url("open_bench", std::env::args().nth(1)) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("open_bench: {e}");
            std::process::exit(2);
        }
    };

    let mpv = libmpv2::Mpv::with_initializer(|init| {
        init.set_property("vo", "null".to_owned())?;
        init.set_property("ao", "null".to_owned())?;
        init.set_property("cache", "yes".to_owned())?;
        init.set_property("demuxer-max-back-bytes", 512i64 * 1024 * 1024)?;
        init.set_property("demuxer-max-bytes", 256i64 * 1024 * 1024)?;
        init.set_property("hwdec", "auto-safe".to_owned())?;
        init.set_property("hr-seek", "yes".to_owned())?;
        init.set_property("user-agent", "VLC/3.0.20 LibVLC/3.0.20".to_owned())?;
        init.set_property("network-timeout", 8i64)?;
        Ok(())
    })
    .expect("mpv");

    let opened = Instant::now();
    mpv.command("loadfile", &[&url, "replace"]).expect("loadfile");

    println!("  t      playhead   behind_live   note");
    let mut first_frame: Option<Duration> = None;
    let mut last_pos = f64::NAN;
    let mut last_tick = Instant::now();
    let mut worst_stall = Duration::ZERO;
    let mut breaches = 0u32;
    let mut peak_behind: f64 = 0.0;
    // The replacement rule, simulated alongside: time actually lost, with
    // the total clamped at zero so jitter cannot ratchet.
    let mut drift: f64 = 0.0;
    let mut drift_catchups = 0u32;
    let mut peak_drift: f64 = 0.0;

    while opened.elapsed() < WATCH {
        std::thread::sleep(TICK);
        while mpv.wait_event(0.0).is_some() {}

        let pos: f64 = mpv.get_property("time-pos").unwrap_or(f64::NAN);
        let behind = behind_live(&mpv).unwrap_or(f64::NAN);

        if first_frame.is_none() && pos.is_finite() && pos > 0.0 {
            first_frame = Some(opened.elapsed());
        }
        if behind.is_finite() && behind > peak_behind {
            peak_behind = behind;
        }

        let mut note = String::new();
        // Would the app have seeked here?
        if behind.is_finite() && behind > FOLLOW_SLACK {
            breaches += 1;
            note.push_str("OVER SLACK - the app would seek forward here");
        }
        // Did playback stop moving?
        if pos.is_finite() && last_pos.is_finite() {
            let advanced = pos - last_pos;
            let elapsed = last_tick.elapsed().as_secs_f64();
            if advanced < elapsed * 0.25 {
                if last_tick.elapsed() > worst_stall {
                    worst_stall = last_tick.elapsed();
                }
                if !note.is_empty() {
                    note.push_str("; ");
                }
                note.push_str("not advancing");
            }
        }
        if pos.is_finite() && last_pos.is_finite() {
            let elapsed = last_tick.elapsed().as_secs_f64();
            let advanced = pos - last_pos;
            if advanced >= 0.0 && advanced <= elapsed * 4.0 {
                drift = (drift + (elapsed - advanced)).max(0.0);
            }
            if drift > peak_drift {
                peak_drift = drift;
            }
            if drift > 10.0 {
                drift_catchups += 1;
                drift = 0.0;
                note.push_str(" [new rule would catch up]");
            }
        }
        if pos.is_finite() {
            last_pos = pos;
            last_tick = Instant::now();
        }

        println!(
            "  {:>5.1}s  {:>8.2}  {:>11}   {}",
            opened.elapsed().as_secs_f64(),
            pos,
            if behind.is_finite() {
                format!("{behind:.2}")
            } else {
                "-".to_owned()
            },
            note
        );
    }

    println!(
        "\n  first frame at {:.2}s",
        first_frame.map(|d| d.as_secs_f64()).unwrap_or(f64::NAN)
    );
    println!(
        "  OLD rule (chase behind_live > {}s): peak behind_live {:.1}s, {} ticks would seek",
        FOLLOW_SLACK, peak_behind, breaches
    );
    println!(
        "  NEW rule (lost playback time > 10s): peak drift {:.2}s, {} catch-ups",
        peak_drift, drift_catchups
    );
    std::process::exit(0);
}
