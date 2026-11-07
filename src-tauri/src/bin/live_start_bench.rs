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
//!
//! `LIVE_START_ONLY=burst` runs only the scenarios with that in their name,
//! and `LIVE_START_PORT=19300` serves from that port up (one per scenario)
//! rather than from any free one, and `LIVE_START_FLAGS="--burst 3"` runs
//! the one server given those flags instead of the list.
//!
//! The last columns: how far behind it was when the start or the catch-up
//! decided (starred when that depth was more than the bytes received could
//! be, a jump in the timestamps, and so was not skipped), how much of the
//! stream played while it was watched, and whether the video track stayed.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use zapper_lib::ipc::live_start::{Join, Reading, Settle};
use zapper_lib::ipc::player_cmds::LIVE_CUSHION;
use zapper_lib::surface::mpv::NO_RECONNECT;

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
    /// How far behind live it was when the start or the catch-up decided.
    decided_behind: Option<f64>,
    /// When the video track went away after the first frame, if it did.
    video_lost: Option<f64>,
    /// Seconds of the stream that played, seeks aside, once it was moving.
    played: f64,
    /// Whether the start or the catch-up found a depth its bytes could not
    /// be (a jump in the timestamps), and so did not skip it.
    phantom: bool,
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
        "{:32} {:8} {:>6} {:>7} {:>8} {:>13} {:>8} {:>8} {:>7} {:>11}",
        "server",
        "mode",
        "frame",
        "moving",
        "behind",
        "then jumped",
        "at live",
        "decided",
        "played",
        "video"
    );
    let only = std::env::var("LIVE_START_ONLY").ok();
    let mut next_port = std::env::var("LIVE_START_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok());
    // `LIVE_START_FLAGS="--burst 3"`: that server alone, instead of the list.
    let custom = std::env::var("LIVE_START_FLAGS").ok();
    let custom: Vec<(String, Vec<&str>)> = custom
        .as_deref()
        .map(|f| vec![(f.to_owned(), f.split_whitespace().collect())])
        .unwrap_or_default();
    let scenarios: Vec<(&str, &[&str])> = if custom.is_empty() {
        SCENARIOS.to_vec()
    } else {
        custom.iter().map(|(n, f)| (n.as_str(), f.as_slice())).collect()
    };
    for (name, flags) in scenarios {
        if only.as_deref().is_some_and(|o| !name.contains(o)) {
            continue;
        }
        let port = match next_port.as_mut() {
            Some(p) => {
                *p += 1;
                Ok(*p - 1)
            }
            None => free_port(),
        };
        let port = match port {
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
                "{:32} {:8} {:>6} {:>7} {:>8} {:>13} {:>8} {:>8} {:>7} {:>11}",
                name,
                if mode == Mode::CatchUp { "catch up" } else { "wait" },
                f(r.first_frame),
                f(r.moving),
                f(r.start_behind),
                r.jump.map_or("-".to_owned(), |(t, by)| format!("{by:.1}s at {t:.1}")),
                f(r.live_at()),
                match (r.decided_behind, r.phantom) {
                    (Some(d), true) => format!("{d:.0}*"),
                    (d, _) => f(d),
                },
                format!("{:.1}", r.played),
                r.video_lost
                    .map_or("kept".to_owned(), |t| format!("lost at {t:.1}")),
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
/// of the seekable window, the end of the cache, where playback is, and the
/// bytes buffered; and whether it is paused.
fn read(mpv: &libmpv2::Mpv) -> (Reading, bool) {
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
    let bytes = |k: &str| v.as_ref().and_then(|v| v.get(k)).and_then(serde_json::Value::as_i64);
    let reading = Reading {
        behind: match (end, at) {
            (Some(e), Some(p)) => (e - p).max(0.0),
            _ => 0.0,
        },
        cache_end,
        bytes_ahead: bytes("fw-bytes"),
        bytes_total: bytes("total-bytes"),
    };
    (reading, mpv.get_property("pause").unwrap_or(false))
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
    let mut last_pos: Option<f64> = None;
    let mut last_tick = Instant::now();
    loop {
        if opened.elapsed() > Duration::from_secs(40) {
            return Err("no picture, or never started".into());
        }
        if run.moving.is_some_and(|m| secs() > m + WATCH_AFTER.as_secs_f64()) {
            return Ok(run);
        }
        if run.first_frame.is_some() && run.video_lost.is_none() {
            // The track the first frame came from, still there? A seek to
            // where the cache holds no video leaves mpv with the sound alone.
            let codec: Option<String> = mpv.get_property("video-codec").ok();
            if codec.is_none_or(|c| c.is_empty()) {
                run.video_lost = Some(secs());
            }
        }
        let wait = next_tick.saturating_duration_since(Instant::now()).as_secs_f64();
        if let Some(ev) = mpv.wait_event(wait) {
            match ev {
                Ok(libmpv2::events::Event::VideoReconfig) if run.first_frame.is_some() => {
                    if trace {
                        let codec: Option<String> = mpv.get_property("video-codec").ok();
                        println!("      {:5.2} video reconfigured: {codec:?}", secs());
                    }
                }
                Ok(libmpv2::events::Event::VideoReconfig) => {
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
        let (r, paused) = read(&mpv);
        if trace {
            println!(
                "      {:5.2} behind {:6.2} cache-end {:?} ahead {:?} total {:?} paused {}",
                secs(),
                r.behind,
                r.cache_end.map(|e| (e * 100.0).round() / 100.0),
                r.bytes_ahead,
                r.bytes_total,
                paused
            );
        }
        if let Some(j) = join.as_mut() {
            debug_assert!(paused);
            if let Some(by) = j.observe(dt, &r) {
                run.decided_behind = Some(r.behind);
                run.phantom |= j.saw_phantom();
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
            // Playing, not seeking: a heartbeat's worth of advance at most.
            let pos: Option<f64> = mpv.get_property("time-pos").ok();
            if let (Some(was), Some(now)) = (last_pos, pos) {
                run.played += (now - was).clamp(0.0, dt * 1.5);
            }
            last_pos = pos;
        }
        if let Some(s) = settle.as_mut() {
            if let Some(by) = s.observe(dt, &r) {
                run.phantom |= s.saw_phantom();
                settle = None;
                run.decided_behind.get_or_insert(r.behind);
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
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../tools/fake_stream.py");
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
