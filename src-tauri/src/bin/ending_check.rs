//! Proves that a stream a provider ends or refuses is explained, and never
//! reopened behind the viewer's back.
//!
//! Each scenario starts `tools/fake_stream.py` on a free port with the flags
//! that imitate one way a provider behaves at its connection limit, plays it
//! in a headless mpv set up exactly as the app's (`vo=null`, `ao=null`, the
//! same reconnect options and the same warning feed), reads the events
//! through the app's own [`signal_of`], and checks what
//! [`zapper_lib::ipc::ending::classify`] makes of them. It also counts the
//! connections the server saw: one per open, never a quiet second one.
//!
//! Usage: `cargo run --features harness --bin ending_check` (fixtures from
//! `tools/make_fixtures.sh`). Silent, windowless, and local: the only server
//! it talks to is the one it starts.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use zapper_lib::ipc::ending::{classify, Clues, Ending, EndingKind, How};
use zapper_lib::surface::mpv::{signal_of, Signal, NO_RECONNECT};

/// How long one open may take to end before the scenario fails.
const DEADLINE: Duration = Duration::from_secs(45);

struct Scenario {
    name: &'static str,
    flags: &'static [&'static str],
    /// Not served at all: nothing listens on the port.
    nobody: bool,
    /// What each open in turn should end as. Two opens is a person pressing
    /// Retry after the first ended.
    expect: &'static [(EndingKind, Option<u16>)],
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "taken over mid-play",
        flags: &["--end-after", "5"],
        nobody: false,
        expect: &[(EndingKind::Dropped, None)],
    },
    Scenario {
        name: "taken over, then Retry is refused",
        flags: &["--end-after", "4", "--after-end", "509", "--reason", "Max Connections Reached"],
        nobody: false,
        expect: &[(EndingKind::Dropped, None), (EndingKind::Refused, Some(509))],
    },
    Scenario {
        name: "refused with 403",
        flags: &["--refuse", "403"],
        nobody: false,
        expect: &[(EndingKind::Refused, Some(403))],
    },
    // FFmpeg says nothing at all about a 401 that comes without a login
    // challenge — not even at mpv's verbose level, measured — so all that
    // can be told is that the open failed. Kept for the request count.
    Scenario {
        name: "refused with a bare 401",
        flags: &["--refuse", "401"],
        nobody: false,
        expect: &[(EndingKind::Other, None)],
    },
    Scenario {
        name: "a line of text instead of video",
        flags: &["--refuse", "body"],
        nobody: false,
        expect: &[(EndingKind::NotVideo, None)],
    },
    Scenario {
        name: "nobody there",
        flags: &[],
        nobody: true,
        expect: &[(EndingKind::Network, None)],
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
            Ok(e) => println!("ended: {e:?}"),
            Err(e) => {
                eprintln!("FAIL: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    let mut failures = 0;
    for s in SCENARIOS {
        match run(&mpv, s, &fixture) {
            Ok(()) => println!("  ok    {}", s.name),
            Err(e) => {
                println!("  FAIL  {}: {e}", s.name);
                failures += 1;
            }
        }
    }
    if failures == 0 {
        println!("PASS: every ending was explained, and nothing reconnected by itself");
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
        init.set_property("user-agent", "VLC/3.0.20 LibVLC/3.0.20".to_owned())?;
        init.set_property("network-timeout", 8i64)?;
        init.set_property("stream-lavf-o", NO_RECONNECT.to_owned())?;
        init.set_property("ytdl", false)?;
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    zapper_lib::surface::mpv::request_warnings(&mpv);
    Ok(mpv)
}

fn run(mpv: &libmpv2::Mpv, s: &Scenario, fixture: &str) -> Result<(), String> {
    let port = free_port()?;
    let mut server = if s.nobody { None } else { Some(Server::start(port, s.flags)?) };
    let url = format!("http://127.0.0.1:{port}/{fixture}");
    for (i, (kind, status)) in s.expect.iter().enumerate() {
        let got = play(mpv, &url)?;
        println!(
            "        open {}: {:?}{}{}",
            i + 1,
            got.kind,
            got.status.map(|s| format!(" (HTTP {s})")).unwrap_or_default(),
            if got.limit { ", a connection limit" } else { "" }
        );
        if got.kind != *kind || got.status != *status {
            return Err(format!("expected {kind:?} {status:?}, got {got:?}"));
        }
        // Long enough for a quiet reconnect to have happened, had there
        // been one.
        std::thread::sleep(Duration::from_secs(3));
        if let Some(srv) = server.as_mut() {
            let seen = srv.requests();
            if seen != i + 1 {
                let log = srv.log.lock().unwrap_or_else(|e| e.into_inner()).join("\n          ");
                return Err(format!(
                    "{} open(s), but the server saw {seen} request(s):\n          {log}",
                    i + 1
                ));
            }
        }
    }
    Ok(())
}

/// Opens `url`, waits for it to end, and says why.
fn play(mpv: &libmpv2::Mpv, url: &str) -> Result<Ending, String> {
    mpv.command("loadfile", &[url, "replace"]).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut shown = false;
    let mut clues = Clues::default();
    let mut loaded = false;
    while started.elapsed() < DEADLINE {
        let Some(ev) = mpv.wait_event(0.25) else { continue };
        match signal_of(ev) {
            Some(Signal::StartFile) => loaded = true,
            Some(Signal::VideoReconfig) => shown = true,
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
                return Ok(classify(how, shown, clues));
            }
            _ => {}
        }
    }
    let _ = mpv.command("stop", &[]);
    Err(format!("did not end within {}s", DEADLINE.as_secs()))
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

    /// Stream requests the server has answered so far.
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
