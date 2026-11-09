//! Times a backward seek, so "rewind feels slow" becomes a number.
//!
//! Headless: `vo=null`, `ao=null`, no window and no render context. The thing
//! being measured is mpv's demuxer and decoder, not our surface, and leaving
//! the window out makes the figure reproducible and silent.
//!
//! The clock runs from issuing `seek` to mpv's `PlaybackRestart`, which is
//! the event mpv raises once it has decoded and is ready to show the first
//! frame at the new position. That is what the viewer is waiting for.
//!
//! Usage:
//!   tools/fake_stream.py &
//!   cargo run --bin seek_bench -- http://127.0.0.1:8788/sample1080.ts
//!
//! A local file is a poor stand-in - it is fully seekable and has no cache
//! to miss - so point this at the loop server, which serves a fixture as an
//! endless chunked response over real HTTP.

use std::time::{Duration, Instant};

/// Seconds of stream to bank before seeking, so the back buffer is real.
const PREROLL: Duration = Duration::from_secs(20);
/// How far back each trial seeks. The app's own skip button.
const SEEK_BY: f64 = -5.0;
/// Seeks per configuration.
const TRIALS: usize = 5;
/// Gap between trials, to let the cache and the decoder settle.
const BETWEEN: Duration = Duration::from_secs(3);
/// A seek that has not restarted by now is the bug, not the measurement.
const SEEK_DEADLINE: Duration = Duration::from_secs(15);

/// The options every configuration shares: the app's own, minus output.
fn base(init: &libmpv2::MpvInitializer) -> libmpv2::Result<()> {
    init.set_property("vo", "null".to_owned())?;
    init.set_property("ao", "null".to_owned())?;
    init.set_property("cache", "yes".to_owned())?;
    init.set_property("demuxer-max-back-bytes", 512i64 * 1024 * 1024)?;
    init.set_property("demuxer-max-bytes", 256i64 * 1024 * 1024)?;
    init.set_property("user-agent", "VLC/3.0.20 LibVLC/3.0.20".to_owned())?;
    init.set_property("network-timeout", 8i64)?;
    Ok(())
}

/// One thing to measure: a label and the properties that differ from [`base`].
struct Config {
    label: &'static str,
    props: &'static [(&'static str, &'static str)],
}

const CONFIGS: &[Config] = &[
    Config {
        label: "app today (hwdec=auto-safe, hr-seek default)",
        props: &[("hwdec", "auto-safe")],
    },
    Config {
        label: "hr-seek=no",
        props: &[("hwdec", "auto-safe"), ("hr-seek", "no")],
    },
    Config {
        label: "hr-seek=no + hr-seek-framedrop=no",
        props: &[
            ("hwdec", "auto-safe"),
            ("hr-seek", "no"),
            ("hr-seek-framedrop", "no"),
        ],
    },
    Config {
        label: "hr-seek=yes (exact)",
        props: &[("hwdec", "auto-safe"), ("hr-seek", "yes")],
    },
    Config {
        label: "software decode, hr-seek default",
        props: &[("hwdec", "no")],
    },
];

/// Drains events until `PlaybackRestart`, or the deadline. Returns how long.
fn wait_restart(mpv: &libmpv2::Mpv, deadline: Duration) -> Option<Duration> {
    let started = Instant::now();
    while started.elapsed() < deadline {
        // A short wait rather than a poll: this thread has nothing else to do,
        // and spinning would contend with the decoder we are timing.
        match mpv.wait_event(0.02) {
            Some(Ok(libmpv2::events::Event::PlaybackRestart)) => return Some(started.elapsed()),
            Some(Err(e)) => {
                eprintln!("    mpv error while seeking: {e}");
                return None;
            }
            _ => {}
        }
    }
    None
}

/// Runs every trial for one configuration and prints the spread.
fn run(url: &str, cfg: &Config) {
    println!("\n  {}", cfg.label);

    let props: Vec<(String, String)> = cfg
        .props
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    let mpv = match libmpv2::Mpv::with_initializer(|init| {
        base(&init)?;
        for (k, v) in &props {
            init.set_property(k, v.clone())?;
        }
        Ok(())
    }) {
        Ok(m) => m,
        Err(e) => {
            println!("    could not start mpv: {e}");
            return;
        }
    };

    if let Err(e) = mpv.command("loadfile", &[url, "replace"]) {
        println!("    could not load {url}: {e}");
        return;
    }

    // Bank a back buffer. Without this the seek has nothing to seek into and
    // every configuration measures the same empty cache.
    let preroll = Instant::now();
    while preroll.elapsed() < PREROLL {
        if let Some(Err(e)) = mpv.wait_event(0.1) {
            println!("    playback failed during preroll: {e}");
            return;
        }
    }

    let cached: f64 = mpv.get_property("demuxer-cache-duration").unwrap_or(0.0);
    println!("    cached {cached:.1}s forward before seeking");

    let mut times = Vec::new();
    for i in 0..TRIALS {
        let before: f64 = mpv.get_property("playback-time").unwrap_or(f64::NAN);
        let issued = Instant::now();
        if let Err(e) = mpv.command("seek", &[&format!("{SEEK_BY:.3}"), "relative"]) {
            println!("    trial {i}: seek refused: {e}");
            continue;
        }
        match wait_restart(&mpv, SEEK_DEADLINE) {
            Some(d) => {
                let after: f64 = mpv.get_property("playback-time").unwrap_or(f64::NAN);
                println!(
                    "    trial {i}: {:>7.1} ms   {before:.2}s -> {after:.2}s",
                    d.as_secs_f64() * 1000.0
                );
                times.push(d.as_secs_f64() * 1000.0);
            }
            None => println!(
                "    trial {i}: no PlaybackRestart within {}s",
                SEEK_DEADLINE.as_secs()
            ),
        }
        let _ = issued;
        let pause = Instant::now();
        while pause.elapsed() < BETWEEN {
            let _ = mpv.wait_event(0.05);
        }
    }

    if times.is_empty() {
        println!("    no successful seeks");
        return;
    }
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    let worst = times.iter().cloned().fold(f64::MIN, f64::max);
    let best = times.iter().cloned().fold(f64::MAX, f64::min);
    println!("    mean {mean:.0} ms   best {best:.0} ms   worst {worst:.0} ms");
}

fn main() {
    // The fixture server's sample by default; a URL off this machine only
    // with ZAPPER_ALLOW_REAL_ACCOUNT=1 (see zapper_lib::harness).
    let url = match zapper_lib::harness::stream_url("seek_bench", std::env::args().nth(1)) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("seek_bench: {e}");
            std::process::exit(2);
        }
    };
    let only = std::env::args().nth(2);

    println!("seeking {SEEK_BY}s, {TRIALS} trials each, against {url}");
    for cfg in CONFIGS {
        if let Some(filter) = &only {
            if !cfg.label.contains(filter.as_str()) {
                continue;
            }
        }
        run(&url, cfg);
    }
}
