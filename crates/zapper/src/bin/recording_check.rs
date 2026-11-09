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
//! Then a stop in the middle of a recording (recording::lifecycle): a second
//! recording of the backup feed, the service stopped part way through with
//! SIGTERM (on Windows, which has none, `POST /api/shutdown` "now"), which
//! must leave `… - part1.mkv` playable on disk and the recording still
//! `recording`; started again, it carries on in `… - part2.mkv` and
//! finishes Done.
//!
//! Then sharing (recording::sharing, recording::hub), on two more channels
//! with a fixture server each, on the playlist's one stream:
//!
//! - two overlapping recordings of one channel, with *Show recordings to
//!   other programs while they record* on: the fixture server sees exactly
//!   one stream request; while they record, their files are the visible
//!   `… (recording).ts`, never hidden ones; each finishes Done as one
//!   playable `.mkv` with no `.ts` left, and both clocks cover the overlap;
//! - a recording in its padding after on one channel when another
//!   channel's programme starts: its padding gives way at once (a note says
//!   it ended early, and for which), and the other starts within seconds of
//!   its start rather than after a back-off.
//!
//! Then *Jellyfin (.strm)*: a recording's `[RECORDING] ….strm` appears,
//! naming its live stream on this service; a wrong key is refused; the
//! stream sends more than was on disk when it was asked for; the part being
//! recorded stays hidden. The service is killed part way and started again:
//! the `.strm` names the same address, the stream (asked for again) carries
//! on from the first part into the second and ends when the recording does,
//! and what it sent probes as H.264. Finished, the `.strm` is gone, the two
//! `.mkv` parts are there, and the key is answered `410`.
//!
//! Needs python3, ffprobe and ffmpeg (`ZAPPER_PYTHON3`, `ZAPPER_FFPROBE` and
//! `ZAPPER_FFMPEG` name them when `PATH` does not; see `harness::tool`), and
//! the fixtures (tools/make_fixtures.sh, or a folder named by `ZAPPER_FIXTURES`).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use zapper_lib::recording::api::{RecordingDto, RecordingState, ScheduleRequest, WhileRecording};
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
    let (p_two, p_three) = (free_port()?, free_port()?);
    let main = Fixture::start(p_main, &["--end-after", "10", "--after-end", "503"])?;
    let backup = Fixture::start(p_backup, &[])?;
    let two = Fixture::start(p_two, &[])?;
    let three = Fixture::start(p_three, &[])?;

    // The account: a playlist on disk naming the two local feeds.
    let playlist = home.join("fixture.m3u");
    std::fs::write(
        &playlist,
        format!(
            "#EXTM3U\n\
             #EXTINF:-1 tvg-id=\"fixture.one\" group-title=\"Fixtures\",Fixture One\n\
             http://127.0.0.1:{p_main}/{main_fx}\n\
             #EXTINF:-1 tvg-id=\"fixture.one\" group-title=\"Fixtures\",Fixture One Backup\n\
             http://127.0.0.1:{p_backup}/{backup_fx}\n\
             #EXTINF:-1 tvg-id=\"fixture.two\" group-title=\"Fixtures\",Fixture Two\n\
             http://127.0.0.1:{p_two}/{backup_fx}\n\
             #EXTINF:-1 tvg-id=\"fixture.three\" group-title=\"Fixtures\",Fixture Three\n\
             http://127.0.0.1:{p_three}/{main_fx}\n"
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
        let wall = clock_cues(&part.path, 0)?;
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
    drop(service);
    interrupted(p_api, ids.1).await?;
    let (id_two, id_three) = wait_for(Duration::from_secs(5), || {
        Some((channel_id("Fixture Two")?, channel_id("Fixture Three")?))
    })
    .ok_or("the playlist's other channels are missing")?;
    shared(p_api, id_two, &two).await?;
    followed_through_strm(p_api, id_two).await?;
    padding_gives_way(p_api, id_two, id_three, &two, &three).await
}

/// A service on the scratch home, and a client for it.
async fn service_and_client(port: u16) -> Result<(Service, Client), String> {
    let service = Service::start(port)?;
    let token = wait_for(Duration::from_secs(20), zapper_lib::recording::config::read_token)
        .ok_or("no token")?;
    let client = Client::new(&format!("http://127.0.0.1:{port}"), Some(token))
        .map_err(|e| e.to_string())?;
    wait_until(Duration::from_secs(20), || async { client.version().await.is_ok() }).await?;
    Ok((service, client))
}

fn request(stream_id: i64, title: &str, start: i64, stop: i64) -> ScheduleRequest {
    ScheduleRequest {
        stream_id,
        backup_stream_id: Some(stream_id),
        channel_name: None,
        title: title.into(),
        episode_title: None,
        season: None,
        episode: None,
        start,
        stop,
        pad_before: Some(0),
        pad_after: Some(0),
    }
}

/// Waits for every one of `ids` to finish; them, in the same order.
async fn finished(client: &Client, ids: &[i64], limit: Duration) -> Result<Vec<RecordingDto>, String> {
    let deadline = Instant::now() + limit;
    loop {
        if Instant::now() > deadline {
            return Err(format!("recordings {ids:?} did not finish in time"));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        let list = client.list().await.map_err(|e| e.to_string())?;
        let got: Vec<RecordingDto> = ids
            .iter()
            .filter_map(|id| list.iter().find(|r| r.id == *id).cloned())
            .collect();
        if got.len() == ids.len()
            && got
                .iter()
                .all(|r| matches!(r.state, RecordingState::Done | RecordingState::Failed))
        {
            return Ok(got);
        }
    }
}

/// Two overlapping recordings of one channel ride one connection.
async fn shared(port: u16, stream_id: i64, fixture: &Fixture) -> Result<(), String> {
    let (_service, client) = service_and_client(port).await?;
    let mut settings = client.settings().await.map_err(|e| e.to_string())?;
    settings.while_recording = Some(WhileRecording::Visible);
    client.set_settings(&settings).await.map_err(|e| e.to_string())?;
    let before = fixture.requests();

    // A recording holds back what it receives for the hub's first OFF_AIR_WINDOW (20 s), so a
    // placeholder clip never reaches its file: their files show from then on. They overlap
    // after it.
    let start = zapper_lib::recording::now();
    let a = client
        .schedule(&request(stream_id, "Fixture: First", start, start + 40))
        .await
        .map_err(|e| e.to_string())?;
    let b = client
        .schedule(&request(stream_id, "Fixture: Second", start + 10, start + 50))
        .await
        .map_err(|e| e.to_string())?;
    if !a.conflicts.is_empty() || !b.conflicts.is_empty() {
        return Err(format!(
            "the same channel is no conflict: {:?} {:?}",
            a.conflicts, b.conflicts
        ));
    }

    // While both record, their files are there for other programs to see.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if Instant::now() > deadline {
            return Err("the two recordings were never on at once".into());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        let list = client.list().await.map_err(|e| e.to_string())?;
        let on: Vec<&RecordingDto> = list
            .iter()
            .filter(|r| (r.id == a.id || r.id == b.id) && r.state == RecordingState::Recording)
            // A part's size is kept when it closes; the file has it now.
            .filter(|r| {
                r.parts
                    .iter()
                    .any(|p| std::fs::metadata(&p.path).is_ok_and(|m| m.len() > 0))
            })
            .collect();
        if on.len() < 2 {
            continue;
        }
        for r in on {
            let name = file_name(&r.parts[0].path);
            if !name.ends_with(" (recording).ts") || name.starts_with('.') {
                return Err(format!("not visible while recording: {name}"));
            }
            for e in std::fs::read_dir(&r.folder).map_err(|e| e.to_string())?.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.starts_with('.') && n.ends_with(".ts") {
                    return Err(format!("a hidden capture file beside it: {n}"));
                }
            }
        }
        break;
    }

    let done = finished(&client, &[a.id, b.id], Duration::from_secs(90)).await?;
    let requests = fixture.requests() - before;
    println!("shared: {requests} stream request(s) for two recordings");
    if requests != 1 {
        return Err(format!("expected one stream request for both, saw {requests}"));
    }
    let mut spans = Vec::new();
    for r in &done {
        if r.state != RecordingState::Done || r.parts.len() != 1 {
            return Err(format!("{}: {:?} in {:?}", r.title, r.state, r.parts));
        }
        let path = &r.parts[0].path;
        if !path.ends_with(".mkv") || file_name(path).contains("(recording)") {
            return Err(format!("{}: finished as {path}", r.title));
        }
        let leftovers: Vec<String> = std::fs::read_dir(&r.folder)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".ts") || n.ends_with(".clock"))
            .collect();
        if !leftovers.is_empty() {
            return Err(format!("{}: left behind {leftovers:?}", r.title));
        }
        let probe = ffprobe(path)?;
        let wall = clock_cues(path, 1)?;
        println!(
            "{}: {} — {:.1} s, clock {} to {}",
            r.title,
            probe.streams.join(", "),
            probe.duration,
            hms(wall[0]),
            hms(*wall.last().unwrap())
        );
        if probe.duration < 12.0 || !probe.streams.iter().any(|s| s.starts_with("video:h264")) {
            return Err(format!("{} is not playable: {:?}, {:.1} s", r.title, probe.streams, probe.duration));
        }
        spans.push((wall[0], *wall.last().unwrap()));
    }
    // Both hold the ten seconds they share.
    let ((a0, a1), (b0, b1)) = (spans[0], spans[1]);
    if !(b0 >= a0 + 5 && a1 >= b0 + 5 && b1 >= a1 + 5) {
        return Err(format!(
            "the files do not both hold the overlap: {}–{} and {}–{}",
            hms(a0),
            hms(a1),
            hms(b0),
            hms(b1)
        ));
    }
    settings.while_recording = Some(WhileRecording::Hidden);
    client.set_settings(&settings).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// A recording's padding after gives way to another channel's start on an
/// account of one stream.
async fn padding_gives_way(
    port: u16,
    first: i64,
    second: i64,
    fx_first: &Fixture,
    fx_second: &Fixture,
) -> Result<(), String> {
    let (_service, client) = service_and_client(port).await?;
    let start = zapper_lib::recording::now();
    let mut c = request(first, "Fixture: Third", start, start + 10);
    c.pad_after = Some(60);
    let c = client.schedule(&c).await.map_err(|e| e.to_string())?;
    let d = client
        .schedule(&request(second, "Fixture: Fourth", start + 20, start + 35))
        .await
        .map_err(|e| e.to_string())?;
    let warned = client
        .list()
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|r| r.id == c.id)
        .map(|r| r.conflicts)
        .unwrap_or_default();
    println!("conflicts foreseen for {}: {warned:?}", c.title);
    let (before_first, before_second) = (fx_first.requests(), fx_second.requests());
    let done = finished(&client, &[c.id, d.id], Duration::from_secs(150)).await?;
    let (c, d) = (&done[0], &done[1]);
    for n in &c.notes {
        println!("  {}: {n}", c.title);
    }
    for n in &d.notes {
        println!("  {}: {n}", d.title);
    }
    if c.state != RecordingState::Done || d.state != RecordingState::Done {
        return Err(format!("expected both Done: {:?} {:?}", c.state, d.state));
    }
    if !c.notes.iter().any(|n| n.contains("early for “Fixture: Fourth”")) {
        return Err("no note that the padding gave way".into());
    }
    let late = d.parts[0].started - d.start;
    println!("the next programme started {late} s after its start");
    if late > 6 {
        return Err(format!("the next programme waited {late} s for the connection"));
    }
    if c.parts[0].ended.is_none_or(|e| e > d.start + 6) {
        return Err(format!("the padding did not give way: {:?}", c.parts[0]));
    }
    let (r1, r2) = (fx_first.requests() - before_first, fx_second.requests() - before_second);
    if r1 != 1 || r2 != 1 {
        return Err(format!("expected one stream request per channel, saw {r1} and {r2}"));
    }
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

/// The files in `folder` whose names pass `keep`.
fn files_in(folder: &str, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    std::fs::read_dir(folder)
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| keep(&file_name(&p.to_string_lossy())))
                .collect()
        })
        .unwrap_or_default()
}

/// The bytes of the capture files in `folder` so far.
fn captured(folder: &str) -> u64 {
    files_in(folder, |n| n.starts_with('.') && n.ends_with(".ts"))
        .iter()
        .map(|p| std::fs::metadata(p).map_or(0, |m| m.len()))
        .sum()
}

/// Reads a live stream into `out` for at most `limit`; how it ended:
/// `true` when the service ended it.
async fn read_stream(
    resp: reqwest::Response,
    out: &mut Vec<u8>,
    limit: Duration,
) -> Result<bool, String> {
    let mut resp = resp;
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        match tokio::time::timeout_at(deadline, resp.chunk()).await {
            Err(_) => return Ok(false),
            Ok(Ok(Some(c))) => out.extend_from_slice(&c),
            Ok(Ok(None)) => return Ok(true),
            Ok(Err(e)) => return Err(format!("reading the live stream: {e}")),
        }
    }
}

/// A recording in Jellyfin (.strm) mode: its `.strm` appears, its address
/// streams the recording as it grows, through a hard stop of the service
/// and the second part that follows, and ends with the recording; then the
/// `.strm` is gone and the `.mkv` files are there.
async fn followed_through_strm(port: u16, stream_id: i64) -> Result<(), String> {
    let (mut service, client) = service_and_client(port).await?;
    let mut settings = client.settings().await.map_err(|e| e.to_string())?;
    settings.while_recording = Some(WhileRecording::Strm);
    client.set_settings(&settings).await.map_err(|e| e.to_string())?;

    let start = zapper_lib::recording::now();
    let rec = client
        // Long enough that after the restart below, its second part (held back for the hub's
        // first 20 s, OFF_AIR_WINDOW) is written while it still records.
        .schedule(&request(stream_id, "Fixture: Followed", start, start + 60))
        .await
        .map_err(|e| e.to_string())?;
    let strm = wait_for(Duration::from_secs(20), || {
        files_in(&rec.folder, |n| n.starts_with("[RECORDING] ") && n.ends_with(".strm"))
            .into_iter()
            .next()
    })
    .ok_or("no .strm appeared")?;
    let url = std::fs::read_to_string(&strm).map_err(|e| e.to_string())?;
    let url = url.trim().to_string();
    let prefix = format!("http://127.0.0.1:{port}/api/recordings/{}/live?key=", rec.id);
    if !url.starts_with(&prefix) || url.len() < prefix.len() + 32 {
        return Err(format!("the .strm does not name the live stream: {}", file_name(&strm.to_string_lossy())));
    }
    println!("strm: {}", file_name(&strm.to_string_lossy()));
    let http = reqwest::Client::new();
    let wrong = http
        .get(format!("{prefix}{}", "0".repeat(64)))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .status();
    if wrong != 401 {
        return Err(format!("a wrong key got {wrong}"));
    }

    // Read it for a while: what is written after connecting comes too.
    wait_until(Duration::from_secs(20), || async { captured(&rec.folder) > 500_000 }).await?;
    let resp = http.get(&url).send().await.map_err(|e| e.to_string())?;
    let at_connect = captured(&rec.folder);
    if resp.status() != 200
        || resp.headers().get("content-type").is_none_or(|v| v != "video/mp2t")
    {
        return Err(format!("the live stream answered {} {:?}", resp.status(), resp.headers()));
    }
    let mut first = Vec::new();
    let ended = read_stream(resp, &mut first, Duration::from_secs(6)).await?;
    println!("strm: {} bytes in 6 s; {at_connect} on disk at connect", first.len());
    if ended || first.len() as u64 <= at_connect + 100_000 {
        return Err(format!(
            "the stream did not follow the file: {} bytes read, {at_connect} on disk at connect",
            first.len()
        ));
    }
    if !files_in(&rec.folder, |n| n.contains("(recording)")).is_empty() {
        return Err("in .strm mode the part being recorded must stay hidden".into());
    }

    // A hard stop; started again, the recording carries on in a second
    // part, and the .strm names the same address.
    service.stop();
    drop(service);
    let (_service, client) = service_and_client(port).await?;
    let again = std::fs::read_to_string(&strm).map_err(|e| format!("the .strm after a restart: {e}"))?;
    if again.trim() != url {
        return Err("the .strm changed across a restart".into());
    }
    wait_until(Duration::from_secs(45), || async {
        one(&client, rec.id).await.is_ok_and(|r| r.parts.len() >= 2)
    })
    .await?;
    let resp = http.get(&url).send().await.map_err(|e| e.to_string())?;
    let at_connect = captured(&rec.folder);
    let mut whole = Vec::new();
    let ended = read_stream(resp, &mut whole, Duration::from_secs(90)).await?;
    let state = one(&client, rec.id).await?.state;
    println!(
        "strm: {} bytes from the start, ended {ended} with the recording {state:?}; {at_connect} on disk at connect",
        whole.len()
    );
    if !ended || state == RecordingState::Recording {
        return Err("the stream did not end with the recording".into());
    }
    if (whole.len() as u64) <= at_connect + 100_000 {
        return Err("the stream did not carry on past what was on disk".into());
    }
    let done = finished(&client, &[rec.id], Duration::from_secs(90)).await?;
    let done = &done[0];
    let names: Vec<String> = done.parts.iter().map(|p| file_name(&p.path)).collect();
    println!("strm: finished {:?} as {names:?}", done.state);
    if done.state != RecordingState::Done
        || names.len() != 2
        || !names.iter().all(|n| n.ends_with(".mkv") && Path::new(&done.folder).join(n).is_file())
    {
        return Err(format!("expected two .mkv parts: {:?} {names:?}", done.state));
    }
    if strm.exists() || !files_in(&done.folder, |n| n.ends_with(".strm")).is_empty() {
        return Err("the .strm was left behind".into());
    }
    let gone = http.get(&url).send().await.map_err(|e| e.to_string())?.status();
    if gone != 410 {
        return Err(format!("its key after the recording finished got {gone}"));
    }

    // What was streamed plays: every video frame decodes. (Its duration
    // says little: the fixture loops, and the second part's timestamps
    // start elsewhere.)
    let saved = std::env::temp_dir().join(format!("zapper-strm-{}.ts", std::process::id()));
    std::fs::write(&saved, &whole).map_err(|e| e.to_string())?;
    let probe = ffprobe(&saved.to_string_lossy());
    let frames = decoded_frames(&saved.to_string_lossy());
    let _ = std::fs::remove_file(&saved);
    let (probe, frames) = (probe?, frames?);
    println!("strm: streamed {}; {frames} video frames decode", probe.streams.join(", "));
    // Some 30 s of 60 frames a second were recorded; a good part of it is
    // plenty to say it plays.
    if !probe.streams.iter().any(|s| s.starts_with("video:h264")) || frames < 600 {
        return Err(format!("the stream is not playable: {:?}, {frames} frames", probe.streams));
    }
    settings.while_recording = Some(WhileRecording::Hidden);
    client.set_settings(&settings).await.map_err(|e| e.to_string())?;
    Ok(())
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A recording stopped part way by the service stopping, then resumed.
async fn interrupted(port: u16, stream_id: i64) -> Result<(), String> {
    let mut service = Service::start(port)?;
    let token = zapper_lib::recording::config::read_token().ok_or("no token")?;
    let client = Client::new(&format!("http://127.0.0.1:{port}"), Some(token))
        .map_err(|e| e.to_string())?;
    wait_until(Duration::from_secs(20), || async { client.version().await.is_ok() }).await?;
    let start = zapper_lib::recording::now();
    let rec = client
        .schedule(&ScheduleRequest {
            stream_id,
            backup_stream_id: Some(stream_id),
            channel_name: None,
            title: "Fixture: Interrupted".into(),
            episode_title: None,
            season: None,
            episode: None,
            start,
            stop: start + 30,
            pad_before: None,
            pad_after: None,
        })
        .await
        .map_err(|e| e.to_string())?;
    // Some seconds of it on disk: five seconds since it started, and some bytes. Bytes alone
    // vary with how the fixture was encoded, and could stand for less than the three seconds
    // checked below.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let st = client.status().await.map_err(|e| e.to_string())?;
        let long_enough = zapper_lib::recording::now() >= start + 5;
        if long_enough && st.active.iter().any(|a| a.id == rec.id && a.bytes > 2_000_000) {
            if st.idle || st.idle_at.is_none_or(|t| t < start + 30) {
                return Err(format!("recording, but the status says idle {} until {:?}", st.idle, st.idle_at));
            }
            break;
        }
        if Instant::now() > deadline {
            return Err("the second recording never got going".into());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    service.terminate(&client).await?;
    let code = service.wait(Duration::from_secs(60)).ok_or("zapper-server did not stop")?;
    println!("stopped part way (exit {code:?})");

    let db = zapper_lib::recording::store::open(
        &zapper_lib::paths::data_dir().join("recordings.db"),
    )
    .map_err(|e| e.to_string())?;
    let mid = zapper_lib::recording::store::get(&db, rec.id)
        .map_err(|e| e.to_string())?
        .ok_or("the recording vanished")?;
    drop(db);
    if mid.state != RecordingState::Recording {
        return Err(format!("stopped part way, it is {:?}, not still recording", mid.state));
    }
    let first = mid.parts.first().ok_or("no part saved")?;
    if !first.path.ends_with(" - part1.mkv") || !Path::new(&first.path).is_file() {
        return Err(format!("the part so far was not finished into its file: {:?}", mid.parts));
    }
    let probe = ffprobe(&first.path)?;
    println!("part1 as left: {} — {:.1} s", probe.streams.join(", "), probe.duration);
    if probe.duration < 3.0 || !probe.streams.iter().any(|s| s.starts_with("video:h264")) {
        return Err(format!("part1 is not playable: {:?}, {:.1} s", probe.streams, probe.duration));
    }

    // Started again while the programme is on: a second part, then Done.
    let _service = Service::start(port)?;
    wait_until(Duration::from_secs(20), || async { client.version().await.is_ok() }).await?;
    let deadline = Instant::now() + Duration::from_secs(90);
    let done = loop {
        if Instant::now() > deadline {
            return Err("the resumed recording did not finish in time".into());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        let r = client
            .list()
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|r| r.id == rec.id)
            .ok_or("the recording vanished")?;
        if matches!(r.state, RecordingState::Done | RecordingState::Failed) {
            break r;
        }
    };
    let names: Vec<&str> = done
        .parts
        .iter()
        .filter_map(|p| Path::new(&p.path).file_name()?.to_str())
        .collect();
    println!("resumed: {:?}, {names:?}", done.state);
    if done.state != RecordingState::Done
        || names.len() != 2
        || !names[0].ends_with(" - part1.mkv")
        || !names[1].ends_with(" - part2.mkv")
    {
        return Err(format!("expected Done in part1.mkv and part2.mkv: {:?} {names:?}", done.state));
    }
    let probe = ffprobe(&done.parts[1].path)?;
    if probe.duration < 3.0 {
        return Err(format!("part2 is only {:.1} s", probe.duration));
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

/// How many video frames of `path` ffmpeg decodes, failing on a decoding
/// error.
fn decoded_frames(path: &str) -> Result<u64, String> {
    let out = Command::new(zapper_lib::harness::tool("ffmpeg"))
        .args([
            "-v", "error", "-nostdin", "-i", path, "-map", "0:v:0", "-f", "null", "-progress",
            "pipe:1", "-",
        ])
        .output()
        .map_err(|e| format!("running ffmpeg: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ffmpeg could not decode {path}: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix("frame=")?.trim().parse().ok())
        .next_back()
        .ok_or_else(|| format!("ffmpeg decoded no frame of {path}"))
}

/// The clock track's cues as seconds of the day, in order. `slack`: how far
/// one cue may step back. A file that runs through the 20-second fixture's
/// loop (where its timestamps start over) may show a second twice there; a
/// provider's stream has no such loop, so the first check allows none.
fn clock_cues(path: &str, slack: i64) -> Result<Vec<i64>, String> {
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
    if cues.windows(2).any(|w| w[0] - w[1] > slack && w[0] - w[1] < 43_000) {
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

    /// Asks it to stop as a package removal or an installer does: SIGTERM
    /// where there are signals, the API's "now" on Windows.
    async fn terminate(&mut self, client: &Client) -> Result<(), String> {
        #[cfg(unix)]
        {
            let _ = client;
            let ok = Command::new("kill")
                .args(["-TERM", &self.0.id().to_string()])
                .status()
                .map_err(|e| e.to_string())?
                .success();
            if !ok {
                return Err("kill -TERM failed".into());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            use zapper_lib::recording::api::{ShutdownRequest, ShutdownWhen};
            client
                .shutdown(&ShutdownRequest { when: ShutdownWhen::Now, restart: false })
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    }

    /// Waits for it to exit by itself; its exit code.
    fn wait(&mut self, limit: Duration) -> Option<Option<i32>> {
        wait_for(limit, || self.0.try_wait().ok().flatten().map(|s| s.code()))
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
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/fake_stream.py");
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
