//! Times the gap in playback caused by changing mpv's audio filter chain.
//!
//! Turning the compressor on stalls the picture for about a second the first
//! time. There are two plausible reasons and they need different fixes, so
//! this tells them apart:
//!
//!  1. The *filter* has to prime. `loudnorm` looks ahead, so it consumes
//!     input before it will emit anything, and mpv holds video to keep sync.
//!     If that is the cause, every filter with a lookahead costs the same and
//!     it costs the same every time.
//!  2. The *chain* has to be rebuilt, and with it the audio output. That is
//!     paid on the first change and possibly not on later ones, which is
//!     exactly the "first time" in the report.
//!
//! Distinguished by toggling repeatedly: a cost that only appears on the
//! first toggle is (2), one that appears on all of them is (1).
//!
//! The audio device is opened for real — `ao=null` would skip the very
//! reinitialisation being measured — but muted and at zero volume, because
//! this runs unattended.
//!
//! Usage:
//!   tools/fake_stream.py &
//!   cargo run -p zapper-mpv --features harness --bin filter_bench -- http://127.0.0.1:8788/sample1080.ts

use std::time::{Duration, Instant};

/// How long to let playback settle before measuring anything.
const SETTLE: Duration = Duration::from_secs(8);
/// How long to watch after each change.
const OBSERVE: Duration = Duration::from_secs(4);
/// Sampling period. Finer than a frame, so a stall of one frame is visible.
const SAMPLE: Duration = Duration::from_millis(10);

/// The chains to try, in the order they are applied. An empty string clears
/// the filter, which is what the interface does when the toggle goes off.
const LOUDNORM: &str = "loudnorm=I=-14:TP=-1.5:LRA=11";

/// The default run: what the interface does today.
const COLD: &[(&str, &str)] = &[
    ("on  (1st)", LOUDNORM),
    ("off (1st)", ""),
    ("on  (2nd)", LOUDNORM),
    ("off (2nd)", ""),
    ("on  (3rd)", LOUDNORM),
];

/// A do-nothing filter installed first, to see whether the cost is "building
/// any chain at all" or something about loudnorm itself.
const WARMED: &[(&str, &str)] = &[
    ("prime", "anull"),
    ("on  (1st)", LOUDNORM),
    ("off (1st)", "anull"),
    ("on  (2nd)", LOUDNORM),
    ("off (2nd)", "anull"),
    ("on  (3rd)", LOUDNORM),
];

/// loudnorm followed by a resampler back to a fixed rate, so the chain's
/// *output* format never changes and the audio device need not be
/// reconfigured when the filter comes and goes.
const RESAMPLED: &[(&str, &str)] = &[
    ("on  (1st)", "loudnorm=I=-14:TP=-1.5:LRA=11,aresample=48000"),
    ("off (1st)", ""),
    ("on  (2nd)", "loudnorm=I=-14:TP=-1.5:LRA=11,aresample=48000"),
    ("off (2nd)", ""),
    ("on  (3rd)", "loudnorm=I=-14:TP=-1.5:LRA=11,aresample=48000"),
];

/// Watches `playback-time` and returns the longest it went without moving.
///
/// A stall is the thing the viewer sees: the clock stops, and so does the
/// picture. Measured against the wall rather than against the stream, since
/// the stream's own clock is what has stopped.
fn longest_stall(mpv: &libmpv2::Mpv, window: Duration) -> (Duration, Duration) {
    let started = Instant::now();
    let mut last_value: f64 = mpv.get_property("playback-time").unwrap_or(f64::NAN);
    let mut last_move = Instant::now();
    let mut worst = Duration::ZERO;
    let mut first_stall = Duration::ZERO;

    while started.elapsed() < window {
        std::thread::sleep(SAMPLE);
        // Drain events so the queue cannot overflow while we poll.
        while mpv.wait_event(0.0).is_some() {}
        let now: f64 = match mpv.get_property("playback-time") {
            Ok(v) => v,
            Err(_) => continue,
        };
        if (now - last_value).abs() > f64::EPSILON {
            let gap = last_move.elapsed();
            if gap > worst {
                worst = gap;
                if first_stall.is_zero() {
                    first_stall = gap;
                }
            }
            last_value = now;
            last_move = Instant::now();
        }
    }
    // A stall still in progress at the end of the window counts too.
    let trailing = last_move.elapsed();
    if trailing > worst {
        worst = trailing;
    }
    (worst, first_stall)
}

fn main() {
    // The fixture server's sample by default; a URL off this machine only
    // with ZAPPER_ALLOW_REAL_ACCOUNT=1 (see zapper_core::harness).
    let url = match zapper_core::harness::stream_url("filter_bench", std::env::args().nth(1)) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("filter_bench: {e}");
            std::process::exit(2);
        }
    };

    let mpv = match libmpv2::Mpv::with_initializer(|init| {
        init.set_property("vo", "null".to_owned())?;
        // A real audio output, because reinitialising it is half of what is
        // being measured — but silent, because this runs unattended.
        init.set_property("mute", "yes".to_owned())?;
        init.set_property("volume", 0i64)?;
        init.set_property("cache", "yes".to_owned())?;
        init.set_property("demuxer-max-back-bytes", 512i64 * 1024 * 1024)?;
        init.set_property("demuxer-max-bytes", 256i64 * 1024 * 1024)?;
        init.set_property("hwdec", "auto-safe".to_owned())?;
        init.set_property("hr-seek", "yes".to_owned())?;
        init.set_property("user-agent", "VLC/3.0.20 LibVLC/3.0.20".to_owned())?;
        init.set_property("network-timeout", 8i64)?;
        // Scenario "pinned": fix the output rate so no filter can change the
        // format the audio device is running at.
        if matches!(std::env::args().nth(2).as_deref(), Some("pinned") | Some("best")) {
            init.set_property("audio-samplerate", 48000i64)?;
        }
        Ok(())
    }) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("could not start mpv: {e}");
            std::process::exit(1);
        }
    };

    // Scenario "best": ask for the filter before anything is loaded, so
    // whatever is one-time about it is paid with no stream to interrupt.
    if std::env::args().nth(2).as_deref() == Some("best") {
        let t = Instant::now();
        let _ = mpv.set_property("af", LOUDNORM.to_string());
        let _ = mpv.set_property("af", String::new());
        println!("  primed before loadfile in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    }

    // Scenario "warmup": run the filter over generated silence at startup,
    // before any channel exists. The cost is only paid when audio actually
    // flows, so it needs a source — but it does not need to be a *stream*,
    // and silence costs nothing and makes no noise.
    if std::env::args().nth(2).as_deref() == Some("warmup") {
        let t = Instant::now();
        let _ = mpv.command(
            "loadfile",
            &["av://lavfi:anullsrc=r=48000:cl=stereo", "replace"],
        );
        let _ = mpv.set_property("af", LOUDNORM.to_string());
        let warm = Instant::now();
        while warm.elapsed() < Duration::from_secs(6) {
            let _ = mpv.wait_event(0.05);
        }
        let _ = mpv.set_property("af", String::new());
        let _ = mpv.command("stop", &[]);
        println!(
            "  warmed over silence in {:.1} s, with nothing playing",
            t.elapsed().as_secs_f64()
        );
    }

    if let Err(e) = mpv.command("loadfile", &[&url, "replace"]) {
        eprintln!("could not load {url}: {e}");
        std::process::exit(1);
    }

    // Scenario "primed": build the real filter once during the settle, then
    // take it out again. If the expensive part is one-time per process, this
    // pays it while nobody is watching and every later toggle is cheap.
    let primed = std::env::args().nth(2).as_deref() == Some("primed");
    if primed {
        let warm = Instant::now();
        while warm.elapsed() < Duration::from_secs(4) {
            let _ = mpv.wait_event(0.05);
        }
        let t = Instant::now();
        let _ = mpv.set_property("af", LOUDNORM.to_string());
        let (stall, _) = longest_stall(&mpv, Duration::from_secs(6));
        let _ = mpv.set_property("af", String::new());
        println!(
            "  priming during startup cost {:.0} ms (hidden behind \"Opening…\"), took {:.0} ms wall",
            stall.as_secs_f64() * 1000.0,
            t.elapsed().as_secs_f64() * 1000.0,
        );
    }

    println!("settling for {}s against {url}", SETTLE.as_secs());
    let settle = Instant::now();
    while settle.elapsed() < SETTLE {
        let _ = mpv.wait_event(0.05);
    }

    let ao: String = mpv
        .get_property("current-ao")
        .unwrap_or_else(|_| "unknown".to_owned());
    let fmt = |m: &libmpv2::Mpv, k: &str| -> String {
        m.get_property::<String>(k).unwrap_or_else(|_| "?".to_owned())
    };
    println!(
        "audio output: {ao}   device format: {} Hz\n",
        fmt(&mpv, "audio-out-params/samplerate")
    );

    // A baseline first: whatever the stream does on its own, before anything
    // is changed. Without it a 200ms network hiccup reads as a filter cost.
    let (idle, _) = longest_stall(&mpv, OBSERVE);
    println!(
        "  {:<10}  longest stall {:>7.0} ms   (baseline, nothing changed)",
        "idle", idle.as_secs_f64() * 1000.0
    );

    let scenario = std::env::args().nth(2).unwrap_or_else(|| "cold".to_owned());
    // A filter given on the command line replaces loudnorm in the cold
    // sequence, so alternatives can be priced the same way.
    let custom = std::env::args().nth(3);
    let custom_seq: Vec<(&str, &str)>;
    let custom_ref: String;
    if let Some(f) = custom {
        custom_ref = f;
        custom_seq = vec![
            ("on  (1st)", custom_ref.as_str()),
            ("off (1st)", ""),
            ("on  (2nd)", custom_ref.as_str()),
            ("off (2nd)", ""),
            ("on  (3rd)", custom_ref.as_str()),
        ];
    } else {
        custom_seq = Vec::new();
    }
    let sequence: &[(&str, &str)] = if !custom_seq.is_empty() {
        &custom_seq
    } else { match scenario.as_str() {
        "warmed" => WARMED,
        "resampled" => RESAMPLED,
        _ => COLD,
    } };
    println!("  scenario: {scenario}");

    for (label, chain) in sequence {
        let applied = Instant::now();
        if let Err(e) = mpv.set_property("af", chain.to_string()) {
            println!("  {label:<10}  setting af failed: {e}");
            continue;
        }
        let set_took = applied.elapsed();
        let (worst, _) = longest_stall(&mpv, OBSERVE);
        println!(
            "  {:<10}  longest stall {:>7.0} ms   device now {} Hz   (set in {:.1} ms)",
            label,
            worst.as_secs_f64() * 1000.0,
            fmt(&mpv, "audio-out-params/samplerate"),
            set_took.as_secs_f64() * 1000.0,
        );
    }

    println!("\n  (baseline above is the stream's own jitter; subtract it)");
    // Exit without unwinding: mpv's teardown from here is not worth the risk
    // in a one-shot harness.
    std::process::exit(0);
}
