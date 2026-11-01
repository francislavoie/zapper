//! Proves that a stream a provider ends or refuses is explained, and never
//! reopened behind the viewer's back — and that one the network takes away
//! is reopened, but only when the account has a connection to spare.
//!
//! Each scenario starts `tools/fake_stream.py` on a free port with the flags
//! that imitate one way a provider, or the network, behaves; plays it in a
//! headless, silent mpv set up exactly as the app's (`vo=null`, `ao=null`,
//! the same reconnect options and the same warning feed); reads the events
//! through the app's own [`signal_of`] and the cache through the same
//! [`Flow`]; and checks what [`classify`] makes of them. Where the app would
//! reconnect by itself, this does too, through the app's own decisions —
//! [`worth_reconnecting`], [`backoff`], and [`reconnect_verdict`] over the
//! fake server's `player_api.php` — with the back-off a tenth as long, so
//! the run takes a minute rather than ten. It counts the stream requests
//! the server saw: one per open, never a quiet extra one.
//!
//! Usage: `cargo run --features harness --bin ending_check` (fixtures from
//! `tools/make_fixtures.sh`). Silent, windowless, and local: the only server
//! it talks to is the one it starts, plus one name under `.invalid`, which
//! by definition resolves nowhere.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use zapper_lib::connections::{backoff, jitter, reconnect_verdict, xtream_status, Limit, Verdict, SECOND_LOOK};
use zapper_lib::ipc::ending::{classify, worth_reconnecting, Clues, Ending, Flow, How};
use zapper_lib::surface::mpv::{signal_of, Signal, DEMUXER_MAX_BYTES, NO_RECONNECT};

/// How long one open may take to end before the scenario fails.
const DEADLINE: Duration = Duration::from_secs(45);

/// The back-off, scaled: the real waits are 2, 5, 10 and 30 seconds.
const TIME_SCALE: f64 = 0.1;

/// Where the URL points.
#[derive(Clone, Copy)]
enum Target {
    /// The scenario's server.
    Server,
    /// A free port nothing listens on.
    Nobody,
    /// A name that does not resolve.
    NoSuchHost,
    /// The server, spoken to in TLS, which it does not speak.
    Tls,
}

/// Where the account's connection count comes from, for a reconnect.
#[derive(Clone, Copy)]
enum Count {
    /// The server's `player_api.php`, as an Xtream account's.
    Api,
    /// Nothing can be asked; the person said this many (a playlist).
    Declared(u32),
}

struct Scenario {
    name: &'static str,
    flags: &'static [&'static str],
    target: Target,
    /// Reconnect as the app would. Off, each open in `expect` is a person
    /// pressing Retry.
    auto: bool,
    count: Count,
    /// What happens, in order: each open's ending (`network`, `refused 509`,
    /// `played`…, with `(unreached)` when nothing reached a server), and
    /// each reconnect decision (`go`, `in use 1 of 1`, `unsafe`).
    expect: &'static [&'static str],
    /// Stream requests the server should have seen.
    requests: Option<usize>,
}

const fn manual(
    name: &'static str,
    flags: &'static [&'static str],
    expect: &'static [&'static str],
) -> Scenario {
    Scenario {
        name,
        flags,
        target: Target::Server,
        auto: false,
        count: Count::Api,
        expect,
        requests: Some(expect.len()),
    }
}

const SCENARIOS: &[Scenario] = &[
    // The provider: explained, never reopened.
    manual("taken over mid-play", &["--end-after", "5"], &["dropped"]),
    manual(
        "taken over, then Retry is refused",
        &["--end-after", "4", "--after-end", "509", "--reason", "Max Connections Reached"],
        &["dropped", "refused 509"],
    ),
    manual("refused with 403", &["--refuse", "403"], &["refused 403"]),
    // FFmpeg says nothing at all about a 401 that comes without a login
    // challenge — not even at mpv's verbose level, measured — so all that
    // can be told is that the open failed. Kept for the request count.
    manual("refused with a bare 401", &["--refuse", "401"], &["other"]),
    manual("a line of text instead of video", &["--refuse", "body"], &["notvideo"]),
    // The same, with the app's reconnect logic running: none of them is
    // reconnected.
    Scenario {
        auto: true,
        ..manual("taken over mid-play, reconnecting on", &["--end-after", "5"], &["dropped"])
    },
    Scenario {
        auto: true,
        ..manual(
            "refused with 509, reconnecting on",
            &["--refuse", "509", "--reason", "Max Connections Reached"],
            &["refused 509"],
        )
    },
    // A connection reset mid-play reaches mpv as an ordinary end of the
    // stream — at every log level, measured — exactly as a provider ending
    // it does. It cannot be told from a takeover, so it is treated as one.
    Scenario {
        name: "connection reset mid-play reads as a takeover",
        flags: &["--drop-after", "4", "--fault-once"],
        target: Target::Server,
        auto: true,
        count: Count::Api,
        expect: &["dropped"],
        requests: Some(1),
    },
    // The network: reconnected when a connection is free.
    Scenario {
        name: "stalled mid-play, then back",
        flags: &["--stall-after", "4", "--fault-once"],
        target: Target::Server,
        auto: true,
        count: Count::Api,
        expect: &["network", "go", "played"],
        requests: Some(2),
    },
    Scenario {
        name: "stalled, and the reconnect is refused",
        flags: &["--stall-after", "4", "--after-end", "509", "--reason", "Max Connections Reached"],
        target: Target::Server,
        auto: true,
        count: Count::Api,
        expect: &["network", "go", "refused 509"],
        requests: Some(2),
    },
    Scenario {
        name: "stalled, and another device has the one connection",
        flags: &["--stall-after", "4", "--other-devices", "1", "--max-connections", "1"],
        target: Target::Server,
        auto: true,
        count: Count::Api,
        expect: &["network", "in use 1 of 1"],
        requests: Some(1),
    },
    Scenario {
        name: "stalled, on a playlist allowed one stream",
        flags: &["--stall-after", "4"],
        target: Target::Server,
        auto: true,
        count: Count::Declared(1),
        expect: &["network", "unsafe"],
        requests: Some(1),
    },
    Scenario {
        name: "stalled, on a playlist allowed two",
        flags: &["--stall-after", "4", "--fault-once"],
        target: Target::Server,
        auto: true,
        count: Count::Declared(2),
        expect: &["network", "go", "played"],
        requests: Some(2),
    },
    // Never reached anything: reconnected until the allowance runs out.
    Scenario {
        name: "nobody there",
        flags: &[],
        target: Target::Nobody,
        auto: true,
        count: Count::Declared(1),
        expect: &[
            "network (unreached)", "go",
            "network (unreached)", "go",
            "network (unreached)", "go",
            "network (unreached)", "go",
            "network (unreached)",
        ],
        requests: None,
    },
    Scenario {
        name: "no such host",
        flags: &[],
        target: Target::NoSuchHost,
        auto: false,
        count: Count::Declared(1),
        expect: &["network (unreached)"],
        requests: None,
    },
    Scenario {
        name: "TLS to a server that does not speak it",
        flags: &[],
        target: Target::Tls,
        auto: false,
        count: Count::Api,
        expect: &["network"],
        requests: Some(0),
    },
];

fn main() {
    // Local by construction, but isolated all the same: nothing here should
    // be able to find an account even by accident.
    zapper_lib::harness::isolate("ending_check");
    let fixture = match fixture() {
        Some(f) => f,
        None => {
            eprintln!("FAIL: no fixtures; run tools/make_fixtures.sh first");
            std::process::exit(2);
        }
    };
    let mpv = match player() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("FAIL: mpv: {e}");
            std::process::exit(2);
        }
    };

    // One URL of your own (on this machine): play it once and say how it
    // ended, for a server these scenarios do not imitate.
    if let Some(url) = std::env::args().nth(1) {
        if let Err(e) = zapper_lib::harness::local_url(&url) {
            eprintln!("FAIL: {e}");
            std::process::exit(2);
        }
        match play(&mpv, &url) {
            Ok(o @ Outcome::Ended(..)) => println!("ended: {}", o.label()),
            Ok(Outcome::Played) => println!("played"),
            Err(e) => {
                eprintln!("FAIL: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // `ZAPPER_ENDING_ONLY=reset`: just the scenarios whose name has that in it.
    let only = std::env::var("ZAPPER_ENDING_ONLY").unwrap_or_default();
    let mut failures = 0;
    for s in SCENARIOS.iter().filter(|s| s.name.contains(only.as_str())) {
        match run(&mpv, s, &fixture) {
            Ok(()) => println!("  ok    {}", s.name),
            Err(e) => {
                println!("  FAIL  {}: {e}", s.name);
                failures += 1;
            }
        }
    }
    if failures == 0 {
        println!(
            "PASS: every ending was explained, only the network was reconnected, \
             and only with a connection free"
        );
    } else {
        println!("FAIL: {failures} scenario(s)");
        std::process::exit(1);
    }
}

/// mpv as the app sets it up, minus the picture and the sound.
fn player() -> Result<libmpv2::Mpv, String> {
    let mpv = libmpv2::Mpv::with_initializer(|init| {
        init.set_property("vo", "null".to_owned())?;
        init.set_property("ao", "null".to_owned())?;
        init.set_property("cache", "yes".to_owned())?;
        init.set_property("demuxer-max-bytes", DEMUXER_MAX_BYTES)?;
        init.set_property("user-agent", "VLC/3.0.20 LibVLC/3.0.20".to_owned())?;
        init.set_property("network-timeout", 8i64)?;
        init.set_property("stream-lavf-o", NO_RECONNECT.to_owned())?;
        init.set_property("ytdl", false)?;
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    zapper_lib::surface::mpv::request_warnings(&mpv);
    // `ZAPPER_ENDING_LOG=v` (or `debug`): more of mpv's log than the app
    // asks for, to look for evidence it does not read yet.
    if let Ok(level) = std::env::var("ZAPPER_ENDING_LOG") {
        if let Ok(level) = std::ffi::CString::new(level) {
            // SAFETY: a live handle and a NUL-terminated string mpv copies.
            unsafe { libmpv2_sys::mpv_request_log_messages(mpv.ctx.as_ptr(), level.as_ptr()) };
        }
    }
    Ok(mpv)
}

fn run(mpv: &libmpv2::Mpv, s: &Scenario, fixture: &str) -> Result<(), String> {
    let port = free_port()?;
    let server = match s.target {
        Target::Server | Target::Tls => Some(Server::start(port, s.flags)?),
        Target::Nobody | Target::NoSuchHost => None,
    };
    let url = match s.target {
        Target::Server | Target::Nobody => format!("http://127.0.0.1:{port}/{fixture}"),
        Target::NoSuchHost => format!("http://zapper-ending-check.invalid/{fixture}"),
        Target::Tls => format!("https://127.0.0.1:{port}/{fixture}"),
    };
    let mut seen: Vec<String> = Vec::new();
    let mut made = 0u32;
    loop {
        let outcome = play(mpv, &url)?;
        let label = outcome.label();
        println!("        {label}");
        seen.push(label);
        if seen.len() >= s.expect.len() && !s.auto {
            break;
        }
        let Outcome::Ended(e, unreached) = outcome else { break };
        if !s.auto {
            // A person pressing Retry, after a moment.
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        if !worth_reconnecting(&e, true) {
            break;
        }
        made += 1;
        let Some(wait) = backoff(made, jitter()) else { break };
        std::thread::sleep(wait.mul_f64(TIME_SCALE));
        let mut verdict = reconnect_verdict(limit(s.count, port), unreached, false);
        if verdict == Verdict::LookAgain {
            std::thread::sleep(SECOND_LOOK.mul_f64(TIME_SCALE * 5.0));
            verdict = reconnect_verdict(limit(s.count, port), unreached, true);
        }
        let label = match verdict {
            Verdict::Go => "go".to_owned(),
            Verdict::InUse(sl) => format!("in use {} of {}", sl.active, sl.max),
            Verdict::Unsafe => "unsafe".to_owned(),
            Verdict::NotYet => "not yet".to_owned(),
            Verdict::LookAgain => "look again".to_owned(),
        };
        println!("        {label}");
        seen.push(label);
        match verdict {
            Verdict::Go => {}
            Verdict::NotYet => continue,
            _ => break,
        }
        if seen.len() > s.expect.len() + 2 {
            break;
        }
    }
    if seen != s.expect {
        return Err(format!("expected {:?}, got {seen:?}", s.expect));
    }
    // Long enough for a quiet reconnect to have happened, had there been one.
    std::thread::sleep(Duration::from_secs(2));
    if let (Some(srv), Some(want)) = (server.as_ref(), s.requests) {
        let got = srv.requests();
        if got != want {
            let log = srv.log.lock().unwrap_or_else(|e| e.into_inner()).join("\n          ");
            return Err(format!("expected {want} stream request(s), the server saw {got}:\n          {log}"));
        }
    }
    Ok(())
}

/// The account's connections, as the app would ask for them.
fn limit(count: Count, port: u16) -> Limit {
    match count {
        Count::Declared(n) => Limit::Declared(n),
        Count::Api => match user_info(port) {
            Some((max, active)) => xtream_status(Some(max), Some(active), None).limit(),
            None => Limit::Unknown,
        },
    }
}

/// `player_api.php`'s `max_connections` and `active_cons`, over a plain
/// socket: the harness has no HTTP client of its own.
fn user_info(port: u16) -> Option<(i64, i64)> {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    s.write_all(b"GET /player_api.php?username=u&password=p HTTP/1.0\r\nHost: x\r\n\r\n").ok()?;
    let mut body = String::new();
    s.read_to_string(&mut body).ok()?;
    let json = &body[body.find("\r\n\r\n")? + 4..];
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let n = |k: &str| {
        let x = v.get("user_info")?.get(k)?;
        x.as_i64().or_else(|| x.as_str()?.parse().ok())
    };
    Some((n("max_connections")?, n("active_cons")?))
}

enum Outcome {
    /// It ended, and this is why; and whether nothing ever reached a server.
    Ended(Ending, bool),
    /// It showed a picture and kept playing for a while: stopped here.
    Played,
}

impl Outcome {
    fn label(&self) -> String {
        match self {
            Outcome::Played => "played".to_owned(),
            Outcome::Ended(e, unreached) => format!(
                "{}{}{}",
                format!("{:?}", e.kind).to_lowercase(),
                e.status.map(|s| format!(" {s}")).unwrap_or_default(),
                if *unreached { " (unreached)" } else { "" }
            ),
        }
    }
}

/// How long a stream must play to count as having come back.
const PLAYED_FOR: Duration = Duration::from_secs(6);

/// Opens `url`, waits for it to end or to play for a while, and says which.
fn play(mpv: &libmpv2::Mpv, url: &str) -> Result<Outcome, String> {
    mpv.command("loadfile", &[url, "replace"]).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut shown: Option<Instant> = None;
    // Where playback was at the first frame, to measure progress from.
    let mut first_pos: Option<f64> = None;
    let mut clues = Clues::default();
    let mut flow = Flow::default();
    let mut loaded = false;
    let mut last = Instant::now();
    while started.elapsed() < DEADLINE {
        let pos: Option<f64> = shown.and_then(|_| mpv.get_property("time-pos").ok());
        if first_pos.is_none() {
            first_pos = pos;
        }
        // Played on, not merely on screen: a stalled stream sits on its
        // picture for as long as the network timeout.
        if pos.zip(first_pos).is_some_and(|(p, f)| p - f >= PLAYED_FOR.as_secs_f64()) {
            let _ = mpv.command("stop", &[]);
            drain(mpv);
            return Ok(Outcome::Played);
        }
        // The cache, a heartbeat at a time, as the app reads it.
        if last.elapsed() >= Duration::from_millis(250) {
            let dt = last.elapsed().as_secs_f64();
            last = Instant::now();
            let (end, eof, fw) = cache(mpv);
            flow.observe(dt, end, eof, fw);
        }
        let Some(ev) = mpv.wait_event(0.05) else { continue };
        match signal_of(ev) {
            Some(Signal::StartFile) => loaded = true,
            Some(Signal::VideoReconfig) => {
                shown.get_or_insert_with(Instant::now);
            }
            Some(Signal::Log(text)) => {
                // Safe to print here, unlike in the app: the only URLs this
                // harness opens are its own server's.
                if std::env::var_os("ZAPPER_ENDING_DEBUG").is_some() {
                    println!("        mpv: {}", text.trim_end());
                }
                clues.observe(&text);
            }
            Some(Signal::EndFile { replaced: true, .. }) => {}
            Some(Signal::EndFile { error, .. }) if loaded => {
                let how = error.map_or(How::Eof, How::Error);
                let mut c = clues;
                c.stalled = flow.stalled();
                let e = classify(how, shown.is_some(), c);
                return Ok(Outcome::Ended(e, c.unreached && shown.is_none()));
            }
            _ => {}
        }
    }
    let _ = mpv.command("stop", &[]);
    Err(format!("did not end within {}s", DEADLINE.as_secs()))
}

/// Swallows the events of a stop, so the next open starts clean.
fn drain(mpv: &libmpv2::Mpv) {
    let until = Instant::now() + Duration::from_millis(500);
    while Instant::now() < until {
        if mpv.wait_event(0.05).is_none() {
            continue;
        }
    }
}

/// The end of the cache, whether the demuxer has read the stream's end, and
/// the bytes ahead: what `Player::probe` hands `Flow` in the app.
fn cache(mpv: &libmpv2::Mpv) -> (Option<f64>, bool, Option<i64>) {
    let Ok(raw) = mpv.get_property::<String>("demuxer-cache-state") else {
        return (None, false, None);
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return (None, false, None);
    };
    (
        v.get("cache-end").and_then(serde_json::Value::as_f64),
        v.get("eof").and_then(serde_json::Value::as_bool).unwrap_or(false),
        v.get("fw-bytes").and_then(serde_json::Value::as_i64),
    )
}

/// `sample1080.ts` if it was generated, else whichever fixture there is.
fn fixture() -> Option<String> {
    let dir = std::env::var_os("ZAPPER_FIXTURES")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".cache/zapper/fixtures")))?;
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".ts"))
        .collect();
    names.sort();
    names
        .iter()
        .find(|n| *n == "sample1080.ts")
        .or_else(|| names.first())
        .cloned()
}

fn free_port() -> Result<u16, String> {
    let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    l.local_addr().map(|a| a.port()).map_err(|e| e.to_string())
}

/// `tools/fake_stream.py`, run for one scenario and stopped with it.
struct Server {
    child: Child,
    log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl Server {
    fn start(port: u16, flags: &[&str]) -> Result<Self, String> {
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../tools/fake_stream.py");
        let mut child = Command::new("python3")
            .arg(script)
            .args(["--port", &port.to_string(), "--verbose"])
            .args(flags)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("starting the fixture server: {e}"))?;
        // Requests are logged on stderr, the server's own lines on stdout;
        // both are kept, and read on threads so neither pipe fills.
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        for pipe in [
            child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
            child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let log = log.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                    log.lock().unwrap_or_else(|e| e.into_inner()).push(line);
                }
            });
        }
        let ready = Instant::now();
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            if ready.elapsed() > Duration::from_secs(10) {
                let _ = child.kill();
                return Err("the fixture server did not start".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // The readiness probe above is a connection but not a request, so it
        // is not counted below.
        Ok(Self { child, log })
    }

    /// Stream requests the server has answered so far. The account's
    /// `player_api.php` is an API call, not a stream, and is not counted.
    fn requests(&self) -> usize {
        self.log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|l| l.contains("\"GET /") && l.contains(".ts"))
            .count()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
