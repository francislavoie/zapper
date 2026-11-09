//! What starting a channel at live costs, and what it saves.
//!
//! A server answers a new connection with a burst of what it already holds,
//! and mpv shows the first frame it gets — the oldest. "Play right away,
//! then catch up to live" lets the picture run from there and jumps it
//! forward once the burst is in ([`Settle`]); "Wait and start at live"
//! holds the first frame instead and starts the picture at live once the
//! burst is in ([`Join`]). This plays
//! the fixture server under several kinds of burst, both ways, through the
//! app's own two state machines, and prints when the picture first moved,
//! how far behind live it was when it did, and any jump after.
//!
//! Usage: `cargo run --features harness --bin live_start_bench [fixture.ts]`
//! (fixtures from `tools/make_fixtures.sh`). Headless and silent: `vo=null`,
//! `ao=null`, and the only server is the one it starts.
//!
//! The fixture should be longer than the burst plus twenty seconds: the
//! server loops it, and the timestamps going back to the start at each loop
//! is a discontinuity no live channel has. One made with
//! `ffmpeg -stream_loop 5 -i motion1080p.ts -c copy long.ts`, in a directory
//! named by `ZAPPER_FIXTURES`, is two minutes of continuous timestamps.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use zapper_lib::player::live_start::{Join, Settle};
use zapper_lib::player::LIVE_CUSHION;
use zapper_lib::player::NO_RECONNECT;

/// The app's heartbeat, so decisions land when the app's would.
const TICK: Duration = Duration::from_millis(250);
/// How long to keep watching after the picture starts, for a late jump.
const WATCH_AFTER: Duration = Duration::from_secs(13);

const SCENARIOS: &[(&str, &[&str])] = &[
    ("no burst", &[]),
    ("no burst, 1.5s gaps every 4s", &["--gap-every", "4"]),
    ("10s burst at once", &["--burst", "10"]),
    ("20s burst at once", &["--burst", "20"]),
    ("30s burst at once", &["--burst", "30"]),
    ("20s burst at 8x", &["--burst", "20", "--burst-rate", "8"]),
    ("30s burst at 8x", &["--burst", "30", "--burst-rate", "8"]),
    ("20s burst at 4x", &["--burst", "20", "--burst-rate", "4"]),
    ("30s burst at 3x", &["--burst", "30", "--burst-rate", "3"]),
];

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// Play from the first frame, catch up after.
    CatchUp,
    /// Hold the first frame until the burst is in.
    Wait,
}

#[derive(Default)]
struct Run {
    first_frame: Option<f64>,
    moving: Option<f64>,
    /// Behind live a second after the picture started moving.
    start_behind: Option<f64>,
    /// A forward jump after the picture was moving: when, and how far.
    jump: Option<(f64, f64)>,
}

impl Run {
    /// When the moving picture first sat at live: at the start, or at the
    /// jump that took it there.
    fn live_at(&self) -> Option<f64> {
        self.jump.map(|(t, _)| t).or(self.moving)
    }
}

fn main() {
    zapper_lib::harness::isolate("live_start_bench");
    let fixture = std::env::args().nth(1).unwrap_or_else(|| "motion1080p.ts".into());
    println!("fixture {fixture}; times in seconds from the open; cushion {LIVE_CUSHION}s\n");
    println!(
        "{:32} {:8} {:>6} {:>7} {:>8} {:>13} {:>8}",
        "server", "mode", "frame", "moving", "behind", "then jumped", "at live"
    );
    for (name, flags) in SCENARIOS {
        let port = match free_port() {
            Ok(p) => p,
            Err(e) => fail(&e),
        };
        let server = match Server::start(port, flags) {
            Ok(s) => s,
            Err(e) => fail(&e),
        };
        let url = format!("http://127.0.0.1:{port}/{fixture}");
        for mode in [Mode::CatchUp, Mode::Wait] {
            let r = match play(&url, mode) {
                Ok(r) => r,
                Err(e) => fail(&format!("{name}: {e}")),
            };
            let f = |v: Option<f64>| v.map_or("-".to_owned(), |v| format!("{v:.2}"));
            println!(
                "{:32} {:8} {:>6} {:>7} {:>8} {:>13} {:>8}",
                name,
                if mode == Mode::CatchUp { "catch up" } else { "wait" },
                f(r.first_frame),
                f(r.moving),
                f(r.start_behind),
                r.jump.map_or("-".to_owned(), |(t, by)| format!("{by:.1}s at {t:.1}")),
                f(r.live_at()),
            );
            // A moment between opens, as a person zapping would leave.
            std::thread::sleep(Duration::from_millis(500));
        }
        drop(server);
    }
}

fn fail(e: &str) -> ! {
    eprintln!("FAIL: {e}");
    std::process::exit(1);
}

/// mpv as the app sets it up, minus the picture and the sound.
fn player() -> Result<libmpv2::Mpv, String> {
    libmpv2::Mpv::with_initializer(|init| {
        init.set_property("vo", "null".to_owned())?;
        init.set_property("ao", "null".to_owned())?;
        init.set_property("cache", "yes".to_owned())?;
        init.set_property("demuxer-max-back-bytes", 512i64 * 1024 * 1024)?;
        init.set_property("demuxer-max-bytes", 256i64 * 1024 * 1024)?;
        init.set_property("hr-seek", "yes".to_owned())?;
        init.set_property("user-agent", "VLC/3.0.20 LibVLC/3.0.20".to_owned())?;
        init.set_property("network-timeout", 8i64)?;
        init.set_property("stream-lavf-o", NO_RECONNECT.to_owned())?;
        init.set_property("ytdl", false)?;
        Ok(())
    })
    .map_err(|e| e.to_string())
}

/// The numbers `Player::probe` derives, from the same properties: the end
/// of the seekable window, the end of the cache, and where playback is.
struct Reading {
    behind: f64,
    cache_end: Option<f64>,
    paused: bool,
}

fn read(mpv: &libmpv2::Mpv) -> Reading {
    let pos: Option<f64> = mpv.get_property("time-pos").ok();
    let raw: Option<String> = mpv.get_property("demuxer-cache-state").ok();
    let v: Option<serde_json::Value> = raw.and_then(|r| serde_json::from_str(&r).ok());
    let get = |k: &str| v.as_ref().and_then(|v| v.get(k)).and_then(serde_json::Value::as_f64);
    let cache_end = get("cache-end");
    let end = v
        .as_ref()
        .and_then(|v| v.get("seekable-ranges"))
        .and_then(|r| r.as_array())
        .and_then(|a| a.last())
        .and_then(|r| r.get("end"))
        .and_then(serde_json::Value::as_f64)
        .or(cache_end);
    let at = get("reader-pts").or(pos);
    Reading {
        behind: match (end, at) {
            (Some(e), Some(p)) => (e - p).max(0.0),
            _ => 0.0,
        },
        cache_end,
        paused: mpv.get_property("pause").unwrap_or(false),
    }
}

fn play(url: &str, mode: Mode) -> Result<Run, String> {
    let mpv = player()?;
    if mode == Mode::Wait {
        mpv.set_property("pause", true).map_err(|e| e.to_string())?;
    }
    let opened = Instant::now();
    let secs = || opened.elapsed().as_secs_f64();
    mpv.command("loadfile", &[url, "replace"]).map_err(|e| e.to_string())?;

    // `LIVE_START_TRACE=1`: every heartbeat's reading, to see why a hold
    // let go when it did.
    let trace = std::env::var_os("LIVE_START_TRACE").is_some();
    let mut run = Run::default();
    let mut join: Option<Join> = None;
    let mut settle: Option<Settle> = None;
    let mut next_tick = Instant::now() + TICK;
    let mut last_tick = Instant::now();
    loop {
        if opened.elapsed() > Duration::from_secs(40) {
            return Err("no picture, or never started".into());
        }
        if run.moving.is_some_and(|m| secs() > m + WATCH_AFTER.as_secs_f64()) {
            return Ok(run);
        }
        let wait = next_tick.saturating_duration_since(Instant::now()).as_secs_f64();
        if let Some(ev) = mpv.wait_event(wait) {
            match ev {
                Ok(libmpv2::events::Event::VideoReconfig) if run.first_frame.is_none() => {
                    run.first_frame = Some(secs());
                    match mode {
                        Mode::CatchUp => {
                            run.moving = run.first_frame;
                            settle = Some(Settle::new());
                        }
                        Mode::Wait => join = Some(Join::new()),
                    }
                }
                Ok(libmpv2::events::Event::EndFile(_)) | Err(_) => {
                    return Err("the stream ended".into());
                }
                _ => {}
            }
            continue;
        }
        // One heartbeat.
        next_tick += TICK;
        let dt = last_tick.elapsed().as_secs_f64();
        last_tick = Instant::now();
        let r = read(&mpv);
        if trace {
            println!(
                "      {:5.2} behind {:6.2} cache-end {:?} paused {}",
                secs(),
                r.behind,
                r.cache_end.map(|e| (e * 100.0).round() / 100.0),
                r.paused
            );
        }
        if let Some(j) = join.as_mut() {
            debug_assert!(r.paused);
            if let Some(by) = j.observe(dt, r.cache_end, r.behind) {
                if by > 0.05 {
                    mpv.command("seek", &[&format!("{by:.3}"), "relative"])
                        .map_err(|e| e.to_string())?;
                }
                mpv.set_property("pause", false).map_err(|e| e.to_string())?;
                run.moving = Some(secs());
                join = None;
                settle = Some(Settle::new());
            }
            continue;
        }
        if let Some(m) = run.moving {
            if run.start_behind.is_none() && secs() >= m + 1.0 {
                run.start_behind = Some(r.behind);
            }
        }
        if let Some(s) = settle.as_mut() {
            if let Some(by) = s.observe(dt, r.behind) {
                settle = None;
                if by > 0.05 {
                    run.jump = Some((secs(), by));
                    mpv.command("seek", &[&format!("{by:.3}"), "relative"])
                        .map_err(|e| e.to_string())?;
                }
            }
        }
    }
}

fn free_port() -> Result<u16, String> {
    let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    l.local_addr().map(|a| a.port()).map_err(|e| e.to_string())
}

/// `tools/fake_stream.py`, for one scenario.
struct Server(Child);

impl Server {
    fn start(port: u16, flags: &[&str]) -> Result<Self, String> {
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/fake_stream.py");
        let mut child = Command::new(zapper_lib::harness::tool("python3"))
            .arg(script)
            .args(["--port", &port.to_string()])
            .args(flags)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("starting the fixture server: {e}"))?;
        if let Some(out) = child.stdout.take() {
            // Drained so the pipe never fills.
            std::thread::spawn(move || for _ in BufReader::new(out).lines() {});
        }
        let ready = Instant::now();
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            if ready.elapsed() > Duration::from_secs(10) {
                let _ = child.kill();
                return Err("the fixture server did not start".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(Self(child))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
