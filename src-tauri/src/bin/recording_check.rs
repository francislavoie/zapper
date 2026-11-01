//! Records from the fixture server through a real `zapper-server`, with a
//! forced switch to the backup feed, and checks what it wrote.
//!
//! ```text
//! cargo build --features harness,server --bin zapper-server --bin recording_check
//! target/debug/recording_check [seconds]
//! ```
//!
//! On a scratch `ZAPPER_HOME` (never the account on this machine): a playlist
//! of two channels carrying the same guide channel, each served by its own
//! `tools/fake_stream.py` — the main one ends its stream after ten seconds
//! and refuses from then on, as a provider handing the connection to another
//! device does. The service syncs that playlist, a recording of the next
//! `seconds` (25 by default) is scheduled through its API, and then:
//!
//! - the recording finishes Done, with a note of the switch;
//! - it is two files, `… - part1.mkv` (main) and `… - part2.mkv` (backup);
//! - each one probes (ffprobe) as Matroska with the video and audio copied,
//!   plus an ASS subtitle stream titled "Clock" that is not a default track;
//! - the clock's cues read the wall time when the part was recorded, and
//!   part 2's begin after part 1's end;
//! - each fixture server saw exactly one stream request: nothing retried a
//!   feed in a loop.
//!
//! Needs python3, ffprobe and ffmpeg (`ZAPPER_PYTHON3`, `ZAPPER_FFPROBE` and
//! `ZAPPER_FFMPEG` name them when `PATH` does not; see `harness::tool`), and
//! the fixtures (tools/make_fixtures.sh, or a folder named by `ZAPPER_FIXTURES`).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use zapper_lib::recording::api::{RecordingDto, RecordingState, ScheduleRequest};
use zapper_lib::recording::client::Client;

fn main() {
    // Before anything reads a path: a scratch home, always.
    if std::env::var_os("ZAPPER_HOME").is_none_or(|v| v.is_empty()) {
        let dir =
            std::env::temp_dir().join(format!("zapper-recording-check-{}", std::process::id()));
        std::env::set_var("ZAPPER_HOME", &dir);
    }
    std::env::remove_var(zapper_lib::harness::ALLOW_REAL_ACCOUNT);
    let seconds: i64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(25);
    let rt = tokio::runtime::Runtime::new().expect("a runtime");
    match rt.block_on(check(seconds)) {
        Ok(()) => println!("recording_check: PASS"),
        Err(e) => {
            eprintln!("recording_check: FAIL: {e}");
            std::process::exit(1);
        }
    }
}

async fn check(seconds: i64) -> Result<(), String> {
    let home = PathBuf::from(std::env::var_os("ZAPPER_HOME").unwrap());
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;
    println!("scratch home: {}", home.display());

    let fixtures = fixtures_dir();
    let pick = |want: &str| -> Result<String, String> {
        if fixtures.join(want).is_file() {
            Ok(want.to_string())
        } else {
            Err(format!(
                "fixture {want} missing from {}; run tools/make_fixtures.sh",
                fixtures.display()
            ))
        }
    };
    let (main_fx, backup_fx) = (pick("motion1080p.ts")?, pick("sport720p60.ts")?);

    let (p_main, p_backup, p_api) = (free_port()?, free_port()?, free_port()?);
    let main = Fixture::start(p_main, &["--end-after", "10", "--after-end", "503"])?;
    let backup = Fixture::start(p_backup, &[])?;

    // The account: a playlist on disk naming the two local feeds.
    let playlist = home.join("fixture.m3u");
    std::fs::write(
        &playlist,
        format!(
            "#EXTM3U\n\
             #EXTINF:-1 tvg-id=\"fixture.one\" group-title=\"Fixtures\",Fixture One\n\
             http://127.0.0.1:{p_main}/{main_fx}\n\
             #EXTINF:-1 tvg-id=\"fixture.one\" group-title=\"Fixtures\",Fixture One Backup\n\
             http://127.0.0.1:{p_backup}/{backup_fx}\n"
        ),
    )
    .map_err(|e| e.to_string())?;
    zapper_lib::catalog::credentials::save_m3u(&zapper_lib::m3u::M3uAccount {
        playlist: playlist.to_string_lossy().into_owned(),
        epg_urls: vec![],
        user_agent: None,
    })
    .map_err(|e| format!("{e:#}"))?;

    let mut service = Service::start(p_api)?;
    let token = wait_for(Duration::from_secs(20), || {
        zapper_lib::recording::config::read_token()
    })
    .ok_or("the service wrote no token")?;
    let client = Client::new(&format!("http://127.0.0.1:{p_api}"), Some(token))
        .map_err(|e| e.to_string())?;
    wait_until(Duration::from_secs(20), || async {
        client.version().await.is_ok()
    })
    .await?;

    // The service syncs the playlist itself.
    let ids = wait_for(Duration::from_secs(60), channel_ids)
        .ok_or("the service did not sync the playlist")?;
    println!("channels: main {} backup {}", ids.0, ids.1);

    let out = home.join("recordings");
    let mut settings = client.settings().await.map_err(|e| e.to_string())?;
    settings.output_dir = out.to_string_lossy().into_owned();
    settings.pad_before = 0;
    settings.pad_after = 0;
    settings.clock_track = true;
    client
        .set_settings(&settings)
        .await
        .map_err(|e| e.to_string())?;

    let start = zapper_lib::recording::now();
    let rec = client
        .schedule(&ScheduleRequest {
            stream_id: ids.0,
            backup_stream_id: None,
            channel_name: None,
            title: "Fixture: Check".into(),
            episode_title: None,
            season: None,
            episode: None,
            start,
            stop: start + seconds,
            pad_before: None,
            pad_after: None,
        })
        .await
        .map_err(|e| e.to_string())?;
    if rec.backup_stream_id != Some(ids.1) {
        return Err(format!(
            "the backup was not found by guide channel: {:?}",
            rec.backup_stream_id
        ));
    }
    println!("scheduled recording {} for {seconds} s", rec.id);

    let deadline = Instant::now() + Duration::from_secs(seconds as u64 + 90);
    let done: RecordingDto = loop {
        if Instant::now() > deadline {
            return Err("the recording did not finish in time".into());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        let list = client.list().await.map_err(|e| e.to_string())?;
        let r = list
            .into_iter()
            .find(|r| r.id == rec.id)
            .ok_or("the recording vanished")?;
        if matches!(r.state, RecordingState::Done | RecordingState::Failed) {
            break r;
        }
    };
    let status = client.status().await.map_err(|e| e.to_string())?;
    service.stop();

    println!("state {:?}, notes:", done.state);
    for n in &done.notes {
        println!("  - {n}");
    }
    if done.state != RecordingState::Done {
        return Err(format!("the recording failed: {:?}", done.error));
    }
    if !done
        .notes
        .iter()
        .any(|n| n.contains("switched from the main feed to the backup"))
    {
        return Err("no note of the switch to the backup feed".into());
    }
    if done.parts.len() != 2 {
        return Err(format!("expected two parts, got {:?}", done.parts));
    }
    let names: Vec<String> = done
        .parts
        .iter()
        .map(|p| {
            Path::new(&p.path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    println!("files: {names:?} (disk free {:?})", status.disk_free);
    if !names[0].starts_with("Fixture - Check - ")
        || !names[0].ends_with(" - part1.mkv")
        || !names[1].ends_with(" - part2.mkv")
    {
        return Err(format!("unexpected names {names:?}"));
    }
    if done.parts[0].stream_id != ids.0 || done.parts[1].stream_id != ids.1 {
        return Err("the parts are not main then backup".into());
    }
    let leftovers: Vec<_> = std::fs::read_dir(&done.folder)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.'))
        .collect();
    if !leftovers.is_empty() {
        return Err(format!("capture files left behind: {leftovers:?}"));
    }

    let mut last_end: Option<i64> = None;
    for (i, part) in done.parts.iter().enumerate() {
        let probe = ffprobe(&part.path)?;
        let wall = clock_cues(&part.path)?;
        println!(
            "part{}: {} — {:.1} s, {} clock cues, {} to {}",
            i + 1,
            probe.streams.join(", "),
            probe.duration,
            wall.len(),
            hms(wall[0]),
            hms(*wall.last().unwrap())
        );
        if !probe.streams.iter().any(|s| s.starts_with("video:h264"))
            || !probe.streams.iter().any(|s| s.starts_with("audio:"))
        {
            return Err(format!(
                "part{} lacks video or audio: {:?}",
                i + 1,
                probe.streams
            ));
        }
        if !probe
            .streams
            .iter()
            .any(|s| s == "subtitle:ass:Clock:not-default")
        {
            return Err(format!(
                "part{} has no non-default Clock track: {:?}",
                i + 1,
                probe.streams
            ));
        }
        if probe.duration < 3.0 {
            return Err(format!("part{} is only {:.1} s", i + 1, probe.duration));
        }
        // The clock reads the wall time it was recorded at (this machine's
        // zone), within the recording's span.
        let first = wall[0];
        let offset = zapper_lib::localtime::utc_offset(start);
        let lo = (start + offset - 15).rem_euclid(86_400);
        let hi = (start + offset + seconds + 15).rem_euclid(86_400);
        let within = |t: i64| {
            if lo <= hi {
                (lo..=hi).contains(&t)
            } else {
                t >= lo || t <= hi
            }
        };
        if !within(first) || !within(*wall.last().unwrap()) {
            return Err(format!(
                "part{}'s clock ({}) is not when it was recorded",
                i + 1,
                hms(first)
            ));
        }
        if let Some(prev) = last_end {
            if first + 1 < prev {
                return Err(format!(
                    "part{}'s clock starts before part{} ended",
                    i + 1,
                    i
                ));
            }
        }
        last_end = Some(*wall.last().unwrap());
    }

    let (rm, rb) = (main.requests(), backup.requests());
    println!("stream requests: main {rm}, backup {rb}");
    if rm != 1 || rb != 1 {
        return Err(format!(
            "expected one request per feed, saw main {rm} and backup {rb}"
        ));
    }
    Ok(())
}

fn hms(t: i64) -> String {
    format!("{:02}:{:02}:{:02}", t / 3600, (t % 3600) / 60, t % 60)
}

fn fixtures_dir() -> PathBuf {
    std::env::var_os("ZAPPER_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".cache/zapper/fixtures")
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

/// The two fixture channels' ids, once the service's sync has written them.
fn channel_ids() -> Option<(i64, i64)> {
    let path = zapper_lib::catalog::db::db_path();
    if !path.exists() {
        return None;
    }
    let c =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    let id = |name: &str| -> Option<i64> {
        c.query_row(
            "SELECT stream_id FROM channels WHERE name = ?1 AND deleted_at IS NULL",
            [name],
            |r| r.get(0),
        )
        .ok()
    };
    Some((id("Fixture One")?, id("Fixture One Backup")?))
}

struct Probe {
    streams: Vec<String>,
    duration: f64,
}

/// `kind:codec[:title:default|not-default]` per stream, and the duration.
fn ffprobe(path: &str) -> Result<Probe, String> {
    let out = Command::new(zapper_lib::harness::tool("ffprobe"))
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
            path,
        ])
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
        .args([
            "-v", "error", "-nostdin", "-i", path, "-map", "0:s:0", "-c", "copy", "-f", "ass", "-",
        ])
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
    if cues.windows(2).any(|w| w[1] < w[0] && w[0] - w[1] < 43_000) {
        return Err(format!("the clock in {path} runs backwards"));
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
            return Err(format!(
                "{} is missing; build it with --bin zapper-server",
                exe.display()
            ));
        }
        let child = Command::new(exe)
            .args(["--port", &port.to_string()])
            .env(
                "RUST_LOG",
                std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
            )
            .spawn()
            .map_err(|e| format!("starting zapper-server: {e}"))?;
        Ok(Self(child))
    }

    fn stop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.stop();
    }
}

/// `tools/fake_stream.py` on `port`, counting the stream requests it answers.
struct Fixture {
    child: Child,
    log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl Fixture {
    fn start(port: u16, flags: &[&str]) -> Result<Self, String> {
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../tools/fake_stream.py");
        let mut child = Command::new(zapper_lib::harness::tool("python3"))
            .arg(script)
            .args(["--port", &port.to_string(), "--verbose"])
            .args(flags)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("starting the fixture server: {e}"))?;
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        for pipe in [
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
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
