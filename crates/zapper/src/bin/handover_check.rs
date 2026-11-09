//! Records what a player is watching on an account of one connection,
//! through a real `zapper-server`, as the app does it (docs/recording.md,
//! "Recording what you watch"; `recording::handover`, `ipc::handover`).
//!
//! ```text
//! cargo build --features harness,server --bin zapper-server --bin handover_check
//! target/debug/handover_check
//! ```
//!
//! On a scratch `ZAPPER_HOME` (never the account on this machine): a
//! playlist of one channel, served by `tools/fake_stream.py`, on an account
//! of one stream. A headless, silent mpv set up as the app's (`vo=null`,
//! `ao=null`) plays it on its own connection for a while, then Record:
//!
//! - the decision is to move the player (`handover::plan`);
//! - what mpv has buffered of the programme is written out with
//!   `dump-cache` (`handover::keep`, `mpv::dump_cache`);
//! - the player lets its connection go and takes a place on the service's
//!   (`POST /api/watch`, the app's token sent as a header), and plays from
//!   it; the recording, scheduled then, rides the same connection, and the
//!   buffer is handed over as its first part (`PUT …/prelude`);
//! - what the service streams to the app is the very bytes the recording
//!   writes (a second place on the connection, read here, is found in the
//!   capture file);
//! - the fixture server sees exactly two stream requests in all (the
//!   player's own, then the service's), and from the hand-over on never
//!   more than one at a time;
//! - the recording finishes Done as two files, `… - part1.mkv` (the
//!   buffer) and `… - part2.mkv` (what the service received), each with
//!   video, audio and a Clock track, the first's clock reading the time the
//!   buffer was received and ending before the second's begins;
//! - after the recording the player still plays, alone on the service's
//!   connection, which closes as soon as it lets go.
//!
//! Needs python3, ffprobe and ffmpeg (`ZAPPER_PYTHON3`, `ZAPPER_FFPROBE`,
//! `ZAPPER_FFMPEG`; see `harness::tool`) and the fixtures
//! (tools/make_fixtures.sh, or a folder named by `ZAPPER_FIXTURES`).

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use zapper_lib::connections::{may_open_another, Limit};
use zapper_lib::recording::api::{RecordingDto, RecordingState, ScheduleRequest};
use zapper_lib::recording::client::{Client, PlayerWatch};
use zapper_lib::recording::handover::{keep, plan, Facts, Plan};
use zapper_lib::recording::{now, now_ms};
use zapper_lib::surface::mpv::{cache_span, dump_cache, DEMUXER_MAX_BYTES, NO_RECONNECT};

/// Seconds the player plays on its own before Record.
const WATCH_FIRST: f64 = 14.0;
/// Seconds recorded from the moment Record is pressed.
const RECORD_FOR: i64 = 25;
const FIXTURE: &str = "sample1080.ts";

fn main() {
    if std::env::var_os("ZAPPER_HOME").is_none_or(|v| v.is_empty()) {
        let dir =
            std::env::temp_dir().join(format!("zapper-handover-check-{}", std::process::id()));
        std::env::set_var("ZAPPER_HOME", &dir);
    }
    std::env::remove_var(zapper_lib::harness::ALLOW_REAL_ACCOUNT);
    let rt = tokio::runtime::Runtime::new().expect("a runtime");
    match rt.block_on(check()) {
        Ok(()) => println!("handover_check: PASS"),
        Err(e) => {
            eprintln!("handover_check: FAIL: {e}");
            std::process::exit(1);
        }
    }
}

async fn check() -> Result<(), String> {
    let home = PathBuf::from(std::env::var_os("ZAPPER_HOME").unwrap());
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;
    println!("scratch home: {}", home.display());
    if !fixtures_dir().join(FIXTURE).is_file() {
        return Err(format!("fixture {FIXTURE} missing; run tools/make_fixtures.sh"));
    }

    let (p_feed, p_api) = (free_port()?, free_port()?);
    let feed = Fixture::start(p_feed)?;
    let playlist = home.join("fixture.m3u");
    std::fs::write(
        &playlist,
        format!(
            "#EXTM3U\n#EXTINF:-1 tvg-id=\"fixture.one\" group-title=\"Fixtures\",Fixture One\n\
             http://127.0.0.1:{p_feed}/{FIXTURE}\n"
        ),
    )
    .map_err(|e| e.to_string())?;
    // A playlist: "Simultaneous streams" says one, which the app counts its
    // player against and the service only its own connections.
    zapper_lib::catalog::credentials::save_m3u(&zapper_lib::m3u::M3uAccount {
        playlist: playlist.to_string_lossy().into_owned(),
        epg_urls: vec![],
        user_agent: None,
    })
    .map_err(|e| format!("{e:#}"))?;

    let _service = Service::start(p_api)?;
    let token = wait_for(Duration::from_secs(20), zapper_lib::recording::config::read_token)
        .ok_or("the service wrote no token")?;
    let client = Client::new(&format!("http://127.0.0.1:{p_api}"), Some(token.clone()))
        .map_err(|e| e.to_string())?;
    wait_until(Duration::from_secs(20), || async { client.version().await.is_ok() }).await?;
    let stream_id = wait_for(Duration::from_secs(60), || channel_id("Fixture One"))
        .ok_or("the service did not sync the playlist")?;
    let mut settings = client.settings().await.map_err(|e| e.to_string())?;
    settings.output_dir = home.join("recordings").to_string_lossy().into_owned();
    settings.clock_track = true;
    client.set_settings(&settings).await.map_err(|e| e.to_string())?;

    // The player, on its own connection.
    let mpv = player()?;
    let direct = format!("http://127.0.0.1:{p_feed}/{FIXTURE}");
    mpv.command("loadfile", &[&direct, "replace"])
        .map_err(|e| format!("loadfile: {e}"))?;
    played(&mpv, WATCH_FIRST, Duration::from_secs(60)).await?;
    let span = cache_span(&mpv).ok_or("mpv reports no cache")?;
    println!("the player has {:.1} s buffered", span.1 - span.0);

    // Record, on a programme that began five minutes ago.
    let t = now();
    let req = ScheduleRequest {
        stream_id,
        backup_stream_id: None,
        channel_name: None,
        title: "Fixture: Watching".into(),
        episode_title: None,
        season: None,
        episode: None,
        start: t - 300,
        stop: t + RECORD_FOR,
        pad_before: Some(60),
        pad_after: Some(0),
    };
    let facts = Facts {
        watching_direct: true,
        starts_now: true,
        room: may_open_another(Limit::Declared(1), 1),
        captured: false,
        service: true,
    };
    if plan(facts) != Plan::Handover {
        return Err(format!("expected to move the player, decided {:?}", plan(facts)));
    }
    let kept = keep(span, now_ms(), req.start - 60, None).ok_or("nothing of the buffer to keep")?;
    let buffer = home.join("buffer.ts");
    let dumped = Instant::now();
    dump_cache(&mpv, kept.start, kept.end, &buffer).map_err(|e| format!("{e:#}"))?;
    let size = std::fs::metadata(&buffer).map(|m| m.len()).unwrap_or(0);
    println!(
        "dump-cache wrote {:.1} s, {} kB, in {} ms",
        kept.end - kept.start,
        size / 1000,
        dumped.elapsed().as_millis()
    );
    if size == 0 {
        return Err("dump-cache wrote nothing".into());
    }

    // The player lets its connection go and takes a place on the service's.
    let before = feed.requests();
    mpv.command("stop", &[]).map_err(|e| e.to_string())?;
    let w = client
        .watch(&PlayerWatch {
            stream_id,
            anyway: true,
            player: true,
            ..PlayerWatch::default()
        })
        .await
        .map_err(|r| format!("no place on the service's connection: {} ({})", r.error, r.reason))?;
    let (url, header) = client.stream(&w);
    if url.contains(&token) {
        return Err("the token is in the stream's address".into());
    }
    mpv.command("change-list", &["http-header-fields", "clr", ""])
        .and_then(|()| {
            mpv.command(
                "change-list",
                &["http-header-fields", "append", header.as_deref().unwrap_or("")],
            )
        })
        .map_err(|e| format!("setting the header: {e}"))?;
    mpv.command("loadfile", &[&url, "replace"])
        .map_err(|e| format!("loadfile: {e}"))?;

    // The recording rides the same connection; the buffer is its first part.
    let rec = client.schedule(&req).await.map_err(|e| e.to_string())?;
    let part = client
        .prelude(rec.id, &buffer, kept.first_ms)
        .await
        .map_err(|e| format!("the buffer was not taken: {e:#}"))?;
    println!("recording {} scheduled; buffer taken as {}", rec.id, file_name(&part.path));
    played(&mpv, 6.0, Duration::from_secs(30))
        .await
        .map_err(|e| format!("from the service: {e}"))?;
    println!("the player plays from the service");

    same_bytes(&client, &token, stream_id, rec.id).await?;

    // One connection from here on, until the recording is done.
    let settled = Instant::now() + Duration::from_secs(2);
    let mut most = 0;
    let done: RecordingDto = loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if Instant::now() > settled {
            most = most.max(active(p_feed).await?);
        }
        let r = client.list().await.map_err(|e| e.to_string())?;
        let r = r.into_iter().find(|r| r.id == rec.id).ok_or("the recording vanished")?;
        if matches!(r.state, RecordingState::Done | RecordingState::Failed) {
            break r;
        }
        if Instant::now() > settled + Duration::from_secs(RECORD_FOR as u64 + 120) {
            return Err("the recording did not finish in time".into());
        }
    };
    let requests = feed.requests() - before;
    println!("stream requests after Record: {requests}; at most {most} at once");
    if requests != 1 || most != 1 {
        return Err(format!(
            "expected one connection for the player and the recording, saw {requests} requests and {most} at once"
        ));
    }

    println!("state {:?}, notes:", done.state);
    for n in &done.notes {
        println!("  - {n}");
    }
    if done.state != RecordingState::Done {
        return Err(format!("the recording failed: {:?}", done.error));
    }
    let names: Vec<String> = done.parts.iter().map(|p| file_name(&p.path)).collect();
    println!("files: {names:?}");
    if names.len() != 2 || !names[0].ends_with(" - part1.mkv") || !names[1].ends_with(" - part2.mkv")
    {
        return Err(format!("expected the buffer and the live part, got {names:?}"));
    }
    let offset = zapper_lib::localtime::utc_offset(t);
    let local = |unix: i64| (unix + offset).rem_euclid(86_400);
    let mut ends = Vec::new();
    for (i, p) in done.parts.iter().enumerate() {
        let probe = ffprobe(&p.path)?;
        let cues = clock_cues(&p.path)?;
        println!(
            "part{}: {} — {:.1} s, clock {} to {}",
            i + 1,
            probe.streams.join(", "),
            probe.duration,
            hms(cues[0]),
            hms(*cues.last().unwrap())
        );
        if !probe.streams.iter().any(|s| s.starts_with("video:h264"))
            || !probe.streams.iter().any(|s| s.starts_with("audio:"))
            || !probe.streams.iter().any(|s| s == "subtitle:ass:Clock:not-default")
        {
            return Err(format!("part{} lacks video, audio or the clock: {:?}", i + 1, probe.streams));
        }
        ends.push((cues[0], *cues.last().unwrap(), probe.duration));
    }
    let want = kept.end - kept.start;
    if (ends[0].2 - want).abs() > 3.0 {
        return Err(format!("part1 is {:.1} s; the buffer kept was {want:.1} s", ends[0].2));
    }
    // The buffer's clock reads when it was received, and ends where the
    // service's part begins (give or take the moments the hand-over took).
    let first = local(kept.first_ms / 1000);
    if (ends[0].0 - first).abs() > 2 {
        return Err(format!(
            "part1's clock starts at {}, but the buffer began arriving at {}",
            hms(ends[0].0),
            hms(first)
        ));
    }
    if ends[0].1 > ends[1].0 + 1 || ends[1].0 - ends[0].1 > 15 {
        return Err(format!(
            "part1's clock ends at {} and part2's begins at {}",
            hms(ends[0].1),
            hms(ends[1].0)
        ));
    }

    // The player stays on the service's connection, alone on it now.
    played(&mpv, 3.0, Duration::from_secs(15))
        .await
        .map_err(|e| format!("after the recording: {e}"))?;
    if active(p_feed).await? != 1 {
        return Err("the player's connection is not the one left open".into());
    }
    client.stop_watching(&w.viewer).await.map_err(|e| format!("{e:#}"))?;
    mpv.command("stop", &[]).map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    while active(p_feed).await? != 0 {
        if t0.elapsed() > Duration::from_secs(5) {
            return Err("the service kept the connection after the player let go".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    println!("let go: the connection closed in {} ms", t0.elapsed().as_millis());
    Ok(())
}

/// A headless, silent mpv with the app's cache and network options.
fn player() -> Result<libmpv2::Mpv, String> {
    libmpv2::Mpv::with_initializer(|init| {
        init.set_property("vo", "null".to_owned())?;
        init.set_property("ao", "null".to_owned())?;
        init.set_property("cache", "yes".to_owned())?;
        init.set_property("demuxer-max-back-bytes", 512i64 * 1024 * 1024)?;
        init.set_property("demuxer-max-bytes", DEMUXER_MAX_BYTES)?;
        init.set_property("network-timeout", 8i64)?;
        init.set_property("stream-lavf-o", NO_RECONNECT.to_owned())?;
        let _ = init.set_property("ytdl", false);
        Ok(())
    })
    .map_err(|e| format!("mpv: {e}"))
}

/// Waits until mpv has played `secs` of the file now open, draining its
/// events as the app's heartbeat does.
async fn played(mpv: &libmpv2::Mpv, secs: f64, limit: Duration) -> Result<(), String> {
    let t = Instant::now();
    let mut first: Option<f64> = None;
    loop {
        while let Some(ev) = mpv.wait_event(0.0) {
            if let Err(e) = ev {
                return Err(format!("mpv: {e}"));
            }
        }
        if let Ok(pos) = mpv.get_property::<f64>("time-pos") {
            let from = *first.get_or_insert(pos);
            if pos - from >= secs {
                return Ok(());
            }
        }
        if t.elapsed() > limit {
            return Err(format!("mpv did not play {secs} s within {} s", limit.as_secs()));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// A second place on the connection, read here with the app's token: a
/// stretch of it must be in the recording's capture file, byte for byte.
async fn same_bytes(client: &Client, token: &str, stream_id: i64, id: i64) -> Result<(), String> {
    let w = client
        .watch(&PlayerWatch {
            stream_id,
            only_shared: true,
            player: true,
            ..PlayerWatch::default()
        })
        .await
        .map_err(|r| format!("a second place: {}", r.error))?;
    if !w.shared {
        return Err("the second place is not on the recording's connection".into());
    }
    let (url, _) = client.stream(&w);
    let http = reqwest::Client::builder().no_proxy().build().map_err(|e| e.to_string())?;
    let mut resp = http
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("reading the stream: {}", resp.status()));
    }
    let mut got = Vec::new();
    while got.len() < 1 << 20 {
        match tokio::time::timeout(Duration::from_secs(10), resp.chunk()).await {
            Ok(Ok(Some(c))) => got.extend_from_slice(&c),
            _ => return Err("the stream stopped".into()),
        }
    }
    drop(resp);
    client.stop_watching(&w.viewer).await.map_err(|e| format!("{e:#}"))?;
    // From well past the head read for the tables, a stretch to find.
    let needle = &got[512 << 10..(512 << 10) + (64 << 10)];
    tokio::time::sleep(Duration::from_secs(1)).await;
    let rec = client
        .list()
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or("the recording vanished")?;
    let live = rec.parts.iter().find(|p| p.ended.is_none()).ok_or("no part being captured")?;
    let mut hay = Vec::new();
    std::fs::File::open(&live.path)
        .and_then(|mut f| f.read_to_end(&mut hay))
        .map_err(|e| format!("reading the capture: {e}"))?;
    if !hay.windows(needle.len()).any(|w| w == needle) {
        return Err("what the service streams is not what the recording writes".into());
    }
    println!(
        "the service's stream is the recording's bytes ({} kB checked in a {} kB capture)",
        needle.len() / 1000,
        hay.len() / 1000
    );
    Ok(())
}

/// The streams the fixture server is sending now, as its account API says.
async fn active(port: u16) -> Result<u32, String> {
    let v: serde_json::Value = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|e| e.to_string())?
        .get(format!("http://127.0.0.1:{port}/player_api.php"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    v["user_info"]["active_cons"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| "the fixture server's account API answered oddly".into())
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn hms(t: i64) -> String {
    format!("{:02}:{:02}:{:02}", t / 3600, (t % 3600) / 60, t % 60)
}

fn fixtures_dir() -> PathBuf {
    std::env::var_os("ZAPPER_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache/zapper/fixtures")
        })
}

fn free_port() -> Result<u16, String> {
    let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    l.local_addr().map(|a| a.port()).map_err(|e| e.to_string())
}

fn wait_for<T>(limit: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let t = Instant::now();
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if t.elapsed() > limit {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

async fn wait_until<F: std::future::Future<Output = bool>>(
    limit: Duration,
    mut f: impl FnMut() -> F,
) -> Result<(), String> {
    let t = Instant::now();
    while !f().await {
        if t.elapsed() > limit {
            return Err("the service did not answer".into());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Ok(())
}

/// A fixture channel's id, once the service's sync has written it.
fn channel_id(name: &str) -> Option<i64> {
    let c = rusqlite::Connection::open_with_flags(
        zapper_lib::catalog::db::db_path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    c.query_row(
        "SELECT stream_id FROM channels WHERE name = ?1 AND deleted_at IS NULL",
        [name],
        |r| r.get(0),
    )
    .ok()
}

struct Probe {
    streams: Vec<String>,
    duration: f64,
}

/// `kind:codec[:title:default|not-default]` per stream, and the duration.
fn ffprobe(path: &str) -> Result<Probe, String> {
    let out = Command::new(zapper_lib::harness::tool("ffprobe"))
        .args(["-v", "error", "-print_format", "json", "-show_streams", "-show_format", path])
        .output()
        .map_err(|e| format!("running ffprobe: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ffprobe could not read {path}: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let streams = v["streams"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|s| {
            let kind = s["codec_type"].as_str().unwrap_or("?");
            let codec = s["codec_name"].as_str().unwrap_or("?");
            if kind == "subtitle" {
                let title = s["tags"]["title"].as_str().unwrap_or("");
                let default = s["disposition"]["default"].as_i64() == Some(1);
                format!(
                    "{kind}:{codec}:{title}:{}",
                    if default { "default" } else { "not-default" }
                )
            } else {
                format!("{kind}:{codec}")
            }
        })
        .collect();
    let duration = v["format"]["duration"]
        .as_str()
        .and_then(|d| d.parse().ok())
        .unwrap_or(0.0);
    Ok(Probe { streams, duration })
}

/// The clock track's cues as seconds of the day, in order.
fn clock_cues(path: &str) -> Result<Vec<i64>, String> {
    let out = Command::new(zapper_lib::harness::tool("ffmpeg"))
        .args(["-v", "error", "-nostdin", "-i", path, "-map", "0:s:0", "-c", "copy", "-f", "ass", "-"])
        .output()
        .map_err(|e| format!("running ffmpeg: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let cues: Vec<i64> = text
        .lines()
        .filter(|l| l.starts_with("Dialogue:"))
        .filter_map(|l| {
            let t = l.rsplit(',').next()?.trim();
            let mut f = t.split(':').map(|x| x.parse::<i64>().ok());
            Some(f.next()?? * 3600 + f.next()?? * 60 + f.next()??)
        })
        .collect();
    if cues.is_empty() {
        return Err(format!("no clock cues in {path}"));
    }
    Ok(cues)
}

/// `zapper-server`, beside this executable, on the scratch home.
struct Service(Child);

impl Service {
    fn start(port: u16) -> Result<Self, String> {
        let exe = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .with_file_name(format!("zapper-server{}", std::env::consts::EXE_SUFFIX));
        if !exe.is_file() {
            return Err(format!("{} is missing; build it with --bin zapper-server", exe.display()));
        }
        let child = Command::new(exe)
            .args(["--port", &port.to_string()])
            .env("RUST_LOG", std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
            .spawn()
            .map_err(|e| format!("starting zapper-server: {e}"))?;
        Ok(Self(child))
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `tools/fake_stream.py` on `port`, counting the stream requests it answers.
struct Fixture {
    child: Child,
    log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl Fixture {
    fn start(port: u16) -> Result<Self, String> {
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/fake_stream.py");
        let mut child = Command::new(zapper_lib::harness::tool("python3"))
            .arg(script)
            .args(["--port", &port.to_string(), "--verbose", "--max-connections", "1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("starting the fixture server: {e}"))?;
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        for pipe in [
            child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>),
            child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>),
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
        Ok(Self { child, log })
    }

    fn requests(&self) -> usize {
        self.log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|l| l.contains("\"GET /") && l.contains(".ts"))
            .count()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
