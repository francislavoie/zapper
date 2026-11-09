//! A channel that is off air, through a real `zapper-server`: the provider
//! answers it with a placeholder clip (`tools/fake_stream.py
//! --placeholder-while`), as some do when a channel is not broadcasting.
//!
//! ```text
//! cargo build -p zapper-server --features harness --bin zapper-server --bin offair_check
//! target/debug/offair_check [base-port] [--serve-until FILE]
//! ```
//!
//! On a scratch `ZAPPER_HOME` (never the account on this machine): a
//! playlist of three channels carrying the same guide channel, the first
//! served by a fixture server that sends the placeholder while a file
//! exists, the other two by an ordinary one; and a fourth channel of its
//! own. Then:
//!
//! - **A recording** of the first, with no backup: it notes "Off air at
//!   HH:MM; retrying" once, asks again on its own (every 25 s here, through
//!   `ZAPPER_OFF_AIR_RETRY_SECS`, which only a harness build reads), and
//!   its capture stays empty however often the clip comes. The file goes,
//!   the channel comes on the air, and the next try records: the recording
//!   finishes Done, one playable part, and the channel's record no longer
//!   says "off air".
//! - **Browsers watching** the first again, once it is off air again: two
//!   viewers on one connection. Each viewer's status says `offAir` with the
//!   clip's length, its stream is over, and the channel's record says "off
//!   air" (`channel_quality`).
//! - **Other sources**: `GET /api/channels/{id}/alternatives` needs a
//!   sign-in, answers the other two as the app's channel rows, and carries
//!   no provider address.
//!
//! With `--serve-until FILE` the service and the fixture servers are left
//! running afterwards, with the sign-in password `offair-check`, until FILE
//! exists: for a browser to look at the web interface.
//!
//! Ports are `base-port` (19700 by default) to `base-port + 2`. Needs
//! python3 and ffprobe (see `harness::tool`) and the fixtures with
//! placeholder.ts (tools/make_fixtures.sh, or `ZAPPER_FIXTURES`).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use zapper_core::recording::api::{RecordingDto, RecordingState, ScheduleRequest};
use zapper_core::recording::client::Client;

/// How long the recording asks again after, here.
const RETRY_SECS: u64 = 25;
/// The recording's length: three off-air tries, then the broadcast.
const RECORD_SECS: i64 = 170;

fn main() {
    if std::env::var_os("ZAPPER_HOME").is_none_or(|v| v.is_empty()) {
        let dir = std::env::temp_dir().join(format!("zapper-offair-check-{}", std::process::id()));
        std::env::set_var("ZAPPER_HOME", &dir);
    }
    std::env::remove_var(zapper_core::harness::ALLOW_REAL_ACCOUNT);
    // Read by the service this starts (a harness build's alone).
    std::env::set_var("ZAPPER_OFF_AIR_RETRY_SECS", RETRY_SECS.to_string());
    let args: Vec<String> = std::env::args().skip(1).collect();
    let base: u16 = args
        .iter()
        .find_map(|a| a.parse().ok())
        .unwrap_or(19700);
    let serve_until = args
        .iter()
        .position(|a| a == "--serve-until")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    let rt = tokio::runtime::Runtime::new().expect("a runtime");
    match rt.block_on(check(base, serve_until)) {
        Ok(()) => println!("offair_check: PASS"),
        Err(e) => {
            eprintln!("offair_check: FAIL: {e}");
            std::process::exit(1);
        }
    }
}

struct Ids {
    lantern: i64,
    hd: i64,
    east: i64,
    other: i64,
}

async fn check(base: u16, serve_until: Option<PathBuf>) -> Result<(), String> {
    let home = PathBuf::from(std::env::var_os("ZAPPER_HOME").unwrap());
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;
    println!("scratch home: {}", home.display());
    let fixtures = fixtures_dir();
    for f in ["placeholder.ts", "motion1080p.ts", "sport720p60.ts"] {
        if !fixtures.join(f).is_file() {
            return Err(format!("fixture {f} missing from {}; run tools/make_fixtures.sh", fixtures.display()));
        }
    }

    let (p_off, p_on, p_api) = (base, base + 1, base + 2);
    let flag = home.join("off-air");
    std::fs::write(&flag, b"").map_err(|e| e.to_string())?;
    let flag_arg = flag.to_string_lossy().into_owned();
    // The clip over about three seconds, as the provider measured sent it.
    let off = Fixture::start(p_off, &["--placeholder-while", &flag_arg, "--burst-rate", "200"])?;
    let _on = Fixture::start(p_on, &[])?;

    let playlist = home.join("fixture.m3u");
    std::fs::write(
        &playlist,
        format!(
            "#EXTM3U\n\
             #EXTINF:-1 tvg-id=\"lantern.one\" tvg-chno=\"1\" group-title=\"Fixtures\",Fixture Lantern\n\
             http://127.0.0.1:{p_off}/motion1080p.ts\n\
             #EXTINF:-1 tvg-id=\"lantern.one\" tvg-chno=\"2\" group-title=\"Fixtures\",Fixture Lantern HD\n\
             http://127.0.0.1:{p_on}/sport720p60.ts\n\
             #EXTINF:-1 tvg-id=\"lantern.one\" tvg-chno=\"3\" group-title=\"Fixtures\",Fixture Lantern East\n\
             http://127.0.0.1:{p_on}/motion1080p.ts\n\
             #EXTINF:-1 tvg-id=\"harbour.one\" tvg-chno=\"4\" group-title=\"Fixtures\",Fixture Harbour\n\
             http://127.0.0.1:{p_on}/sport720p60.ts\n"
        ),
    )
    .map_err(|e| e.to_string())?;
    zapper_core::catalog::credentials::save_m3u(&zapper_core::m3u::M3uAccount {
        playlist: playlist.to_string_lossy().into_owned(),
        epg_urls: vec![],
        user_agent: None,
    })
    .map_err(|e| format!("{e:#}"))?;

    let mut service = Service::start(p_api)?;
    let token = wait_for(Duration::from_secs(20), zapper_core::recording::config::read_token)
        .ok_or("the service wrote no token")?;
    let api = format!("http://127.0.0.1:{p_api}");
    let client = Client::new(&api, Some(token.clone())).map_err(|e| e.to_string())?;
    wait_until(Duration::from_secs(20), || async { client.version().await.is_ok() }).await?;
    let ids = wait_for(Duration::from_secs(60), || {
        Some(Ids {
            lantern: channel_id("Fixture Lantern")?,
            hd: channel_id("Fixture Lantern HD")?,
            east: channel_id("Fixture Lantern East")?,
            other: channel_id("Fixture Harbour")?,
        })
    })
    .ok_or("the service did not sync the playlist")?;
    println!(
        "channels: lantern {} (off air), hd {}, east {}, harbour {}",
        ids.lantern, ids.hd, ids.east, ids.other
    );

    recording(&client, &home, ids.lantern, &off, &flag).await?;

    // Off air again, for the browsers.
    std::fs::write(&flag, b"").map_err(|e| e.to_string())?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    watching(&http, &api, &token, ids.lantern).await?;
    alternatives(&http, &api, &token, &ids, &[p_off, p_on]).await?;

    if let Some(until) = serve_until {
        client
            .set_password("offair-check")
            .await
            .map_err(|e| e.to_string())?;
        println!(
            "serving {api}/ (password offair-check; channel {} off air) until {} exists",
            ids.lantern,
            until.display()
        );
        while !until.exists() {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    service.stop();
    Ok(())
}

/// A recording of a channel that is off air: retried, never grown by the
/// clip, and recorded once the channel is on the air.
async fn recording(
    client: &Client,
    home: &Path,
    stream_id: i64,
    off: &Fixture,
    flag: &Path,
) -> Result<(), String> {
    let out = home.join("recordings");
    let mut settings = client.settings().await.map_err(|e| e.to_string())?;
    settings.output_dir = out.to_string_lossy().into_owned();
    settings.pad_before = 0;
    settings.pad_after = 0;
    client.set_settings(&settings).await.map_err(|e| e.to_string())?;
    let start = zapper_core::recording::now();
    let rec = client
        .schedule(&ScheduleRequest {
            stream_id,
            // Itself: no backup to switch to.
            backup_stream_id: Some(stream_id),
            channel_name: None,
            title: "Fixture: Off Air".into(),
            episode_title: None,
            season: None,
            episode: None,
            start,
            stop: start + RECORD_SECS,
            pad_before: Some(0),
            pad_after: Some(0),
        })
        .await
        .map_err(|e| e.to_string())?;
    println!("recording {} scheduled for {RECORD_SECS} s", rec.id);

    // Three tries, each answered with the clip.
    let t = Instant::now();
    let mut seen_retry = None;
    while off.placeholders() < 3 {
        if t.elapsed() > Duration::from_secs(4 * RETRY_SECS + 60) {
            return Err(format!("only {} tries in {:?}", off.placeholders(), t.elapsed()));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        let r = one(client, rec.id).await?;
        // Between tries: a retry time, never a wait for a connection,
        // which would tell browsers and the app it needs one.
        if let Some(w) = &r.waiting {
            return Err(format!("off air, the recording says it is waiting: {w}"));
        }
        if let Some(at) = r.retry_at {
            seen_retry.get_or_insert(at);
        }
        grows_by_nothing(&r)?;
    }
    // The third try's clip has been sent; give the service a moment to
    // judge it, then look again.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let r = one(client, rec.id).await?;
    grows_by_nothing(&r)?;
    let tries = off.placeholders();
    println!(
        "after {tries} tries, {:.0} s apart on average: nothing captured; notes {:?}; first retryAt {:?}",
        t.elapsed().as_secs_f64() / tries as f64,
        r.notes,
        seen_retry
    );
    let off_air_notes: Vec<&String> = r.notes.iter().filter(|n| n.starts_with("Off air at ")).collect();
    if off_air_notes.len() != 1 || !off_air_notes[0].ends_with("; retrying") {
        return Err(format!("expected one \"Off air at HH:MM; retrying\" note, got {:?}", r.notes));
    }
    if seen_retry.is_none() {
        return Err("the recording never said when it would try again".into());
    }
    if ledger(stream_id).0.as_deref() != Some("off air") {
        return Err(format!("the channel's record does not say off air: {:?}", ledger(stream_id)));
    }

    // On the air: the next try records.
    std::fs::remove_file(flag).map_err(|e| e.to_string())?;
    println!("the channel is on the air from now");
    let deadline = Instant::now() + Duration::from_secs(RECORD_SECS as u64 + 90);
    let done: RecordingDto = loop {
        if Instant::now() > deadline {
            return Err("the recording did not finish in time".into());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        let r = one(client, rec.id).await?;
        if matches!(r.state, RecordingState::Done | RecordingState::Failed) {
            break r;
        }
    };
    println!("state {:?}, error {:?}, notes:", done.state, done.error);
    for n in &done.notes {
        println!("  - {n}");
    }
    if done.state != RecordingState::Done {
        return Err(format!("the recording failed: {:?}", done.error));
    }
    if done.notes.iter().filter(|n| n.starts_with("Off air at ")).count() != 1 {
        return Err("the off-air note was given more than once".into());
    }
    if done.parts.len() != 1 {
        return Err(format!("expected one part, got {:?}", done.parts));
    }
    let probe = ffprobe(&done.parts[0].path)?;
    println!(
        "{}: {:.1} s, {} bytes, {}",
        file_name(&done.parts[0].path),
        probe.1,
        done.parts[0].bytes,
        probe.0.join(", ")
    );
    // The broadcast's own fixture is 1080p; the placeholder's black is too,
    // so its length tells them apart: never ten minutes.
    if probe.1 < 15.0 || probe.1 > RECORD_SECS as f64 {
        return Err(format!("the part is {:.1} s long", probe.1));
    }
    if !probe.0.iter().any(|s| s.starts_with("video:h264")) {
        return Err(format!("the part has no video: {:?}", probe.0));
    }
    let leftovers: Vec<String> = std::fs::read_dir(&done.folder)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.'))
        .collect();
    if !leftovers.is_empty() {
        return Err(format!("capture files left behind: {leftovers:?}"));
    }
    let (reason, _) = ledger(stream_id);
    if reason.is_some() {
        return Err(format!("the channel's record still says {reason:?} after it broadcast"));
    }
    println!("recording: PASS (the channel's record no longer says off air)");
    Ok(())
}

/// The recording holds nothing of the clip: no bytes in any part, and no
/// capture file with anything in it.
fn grows_by_nothing(r: &RecordingDto) -> Result<(), String> {
    let in_parts: u64 = r.parts.iter().map(|p| p.bytes).sum();
    let on_disk = captured(&r.folder);
    if in_parts > 0 || on_disk > 0 {
        return Err(format!(
            "the recording grew while the channel was off air: {in_parts} bytes in its parts, {on_disk} on disk"
        ));
    }
    Ok(())
}

/// Two browsers' viewers on the channel while it is off air.
async fn watching(http: &reqwest::Client, api: &str, token: &str, stream_id: i64) -> Result<(), String> {
    let before = zapper_core::recording::now();
    let start = || async {
        http.post(format!("{api}/api/watch"))
            .bearer_auth(token)
            .json(&serde_json::json!({ "streamId": stream_id }))
            .send()
            .await
            .map_err(|e| e.to_string())
    };
    let mut viewers = Vec::new();
    for i in 0..2 {
        let r = start().await?;
        let status = r.status().as_u16();
        let body: serde_json::Value = r.json().await.map_err(|e| e.to_string())?;
        match status {
            200 => {
                println!("viewer {i}: started (shared: {})", body["shared"]);
                viewers.push(body["viewer"].as_str().unwrap_or_default().to_string());
            }
            // The clip ended before the tables were read: said at once.
            502 if body["ending"]["kind"] == "offAir" => {
                println!("viewer {i}: refused as off air at once: {body}");
            }
            _ => return Err(format!("watching answered {status}: {body}")),
        }
    }
    if viewers.is_empty() {
        return Err("no viewer started, so its status could not be asked".into());
    }
    for v in &viewers {
        let t = Instant::now();
        let said = loop {
            let said: serde_json::Value = http
                .get(format!("{api}/api/watch/{v}"))
                .bearer_auth(token)
                .send()
                .await
                .map_err(|e| e.to_string())?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            if !said["ended"].is_null() {
                break said;
            }
            if t.elapsed() > Duration::from_secs(30) {
                return Err(format!("viewer {v} never ended: {said}"));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        println!("viewer status: {said} ({:.1} s)", t.elapsed().as_secs_f64());
        if said["ended"]["kind"] != "offAir" {
            return Err(format!("the viewer did not end as off air: {said}"));
        }
        let clip = said["ended"]["clipSecs"].as_u64().unwrap_or(0);
        if !(590..=610).contains(&clip) {
            return Err(format!("the clip's length is {clip} s, not about 600"));
        }
        let gone = http
            .get(format!("{api}/api/stream/{v}"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .status()
            .as_u16();
        if gone != 410 {
            return Err(format!("the viewer's stream answered {gone}, not 410"));
        }
    }
    let t = Instant::now();
    loop {
        let (reason, at) = ledger(stream_id);
        if reason.as_deref() == Some("off air") && at.is_some_and(|a| a >= before) {
            println!("channel_quality: fail_reason {reason:?}, failed_at {at:?}");
            break;
        }
        if t.elapsed() > Duration::from_secs(5) {
            return Err(format!("the channel's record says {reason:?} at {at:?}"));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    println!("watching: PASS");
    Ok(())
}

/// The other sources route: signed in only, the app's rows, no provider
/// address.
async fn alternatives(
    http: &reqwest::Client,
    api: &str,
    token: &str,
    ids: &Ids,
    provider_ports: &[u16],
) -> Result<(), String> {
    let url = |id: i64| format!("{api}/api/channels/{id}/alternatives");
    let anon = http
        .get(url(ids.lantern))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .status()
        .as_u16();
    if anon != 401 {
        return Err(format!("without a sign-in the route answered {anon}"));
    }
    let raw = http
        .get(url(ids.lantern))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    println!("alternatives: {raw}");
    let body: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let got: Vec<i64> = body
        .as_array()
        .ok_or("not a list")?
        .iter()
        .filter_map(|c| c["streamId"].as_i64())
        .collect();
    if got != [ids.hd, ids.east] {
        return Err(format!("expected [{}, {}], got {got:?}", ids.hd, ids.east));
    }
    for key in ["name", "cleanName", "num", "logoUrl", "categoryId", "hasArchive", "quality", "nowPlaying"] {
        if body[0].get(key).is_none() {
            return Err(format!("{key} missing from {}", body[0]));
        }
    }
    for port in provider_ports {
        if raw.contains(&format!(":{port}")) {
            return Err(format!("a provider address is in the answer: {raw}"));
        }
    }
    if raw.contains("http://") || raw.contains("https://") || raw.contains(".ts") {
        return Err(format!("an address is in the answer: {raw}"));
    }
    let none = http
        .get(url(ids.other))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    if none.trim() != "[]" {
        return Err(format!("a channel of its own has others: {none}"));
    }
    println!("alternatives: PASS");
    Ok(())
}

/// Recording `id` as the service has it now.
async fn one(client: &Client, id: i64) -> Result<RecordingDto, String> {
    client
        .list()
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| "the recording vanished".to_string())
}

/// The bytes of the capture files in `folder` so far.
fn captured(folder: &str) -> u64 {
    std::fs::read_dir(folder)
        .map(|d| {
            d.flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".ts"))
                .map(|e| e.metadata().map_or(0, |m| m.len()))
                .sum()
        })
        .unwrap_or(0)
}

/// The channel's record: `fail_reason` and `failed_at`.
fn ledger(stream_id: i64) -> (Option<String>, Option<i64>) {
    let Ok(c) = rusqlite::Connection::open_with_flags(
        zapper_core::catalog::db::db_path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return (None, None);
    };
    c.query_row(
        "SELECT fail_reason, failed_at FROM channel_quality WHERE stream_id = ?1",
        [stream_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap_or((None, None))
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The streams in a file (`video:h264`, `audio:aac`, ...) and its length.
fn ffprobe(path: &str) -> Result<(Vec<String>, f64), String> {
    let out = Command::new(zapper_core::harness::tool("ffprobe"))
        .args(["-v", "error", "-show_entries", "format=duration:stream=codec_type,codec_name"])
        .args(["-of", "json", path])
        .output()
        .map_err(|e| format!("ffprobe: {e}"))?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let streams = v["streams"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| {
                    format!(
                        "{}:{}",
                        s["codec_type"].as_str().unwrap_or("?"),
                        s["codec_name"].as_str().unwrap_or("?")
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let duration = v["format"]["duration"]
        .as_str()
        .and_then(|d| d.parse().ok())
        .unwrap_or(0.0);
    Ok((streams, duration))
}

fn fixtures_dir() -> PathBuf {
    std::env::var_os("ZAPPER_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache/zapper/fixtures")
        })
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
        zapper_core::catalog::db::db_path(),
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

/// `tools/fake_stream.py` on `port`, keeping what it logs.
struct Fixture {
    child: Child,
    log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl Fixture {
    fn start(port: u16, flags: &[&str]) -> Result<Self, String> {
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/fake_stream.py");
        let mut child = Command::new(zapper_core::harness::tool("python3"))
            .arg(script)
            .args(["--port", &port.to_string(), "--verbose"])
            .args(flags)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("starting the fixture server: {e}"))?;
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
                    if line.starts_with("placeholder for") {
                        println!("  fake: {line}");
                    }
                    log.lock().unwrap_or_else(|e| e.into_inner()).push(line);
                }
            });
        }
        let ready = Instant::now();
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            if ready.elapsed() > Duration::from_secs(10) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("the fixture server on {port} did not start"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(Self { child, log })
    }

    /// Clips sent whole.
    fn placeholders(&self) -> usize {
        self.log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|l| l.starts_with("placeholder for") && l.ends_with("then closed"))
            .count()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
