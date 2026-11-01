//! Capturing a recording: the stream's bytes, copied to disk as they come,
//! then each part remuxed to Matroska.
//!
//! The stream is fetched the way mpv fetches it when the app plays the
//! channel — the same URL, the same User-Agent and Referer rules — and the
//! bytes go to the part file untouched: nothing is decoded or re-encoded.
//! Beside each part a small log records when its bytes arrived (see
//! [`super::clock`]), which is what the clock track is drawn from.
//!
//! Which feed to connect to, and when, is [`super::fallback`]'s decision;
//! whether the account has a connection to spare is asked of the service's
//! [`super::budget`] before every connection.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;

use super::api::{PartDto, RecordingState};
use super::budget::{jitter, wait_for_slot, Slots};
use super::clock::{self, Arrival};
use super::fallback::{Failure, Fallback, STALL_SECS};
use super::naming;
use super::server::Server;
use super::{now, now_ms, remux, store};
use crate::source::{Playable, Source};

/// The arrival log beside a part: `<part>.clock`.
pub fn log_path(part: &Path) -> PathBuf {
    let mut s = part.as_os_str().to_owned();
    s.push(".clock");
    PathBuf::from(s)
}

/// A part being written.
struct OpenPart {
    stream_id: i64,
    path: PathBuf,
    file: tokio::fs::File,
    log: tokio::fs::File,
    bytes: u64,
    started: i64,
    last_logged_ms: i64,
}

impl OpenPart {
    async fn create(folder: &Path, base: &str, index: usize, stream_id: i64) -> Result<Self> {
        tokio::fs::create_dir_all(folder)
            .await
            .with_context(|| format!("creating {}", folder.display()))?;
        let path = folder.join(naming::capture_file(base, index));
        // Appending: a part resumed after a restart continues its file.
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .with_context(|| format!("creating {}", path.display()))?;
        let bytes = file.metadata().await.map(|m| m.len()).unwrap_or(0);
        let log = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path(&path))
            .await?;
        Ok(Self {
            stream_id,
            path,
            file,
            log,
            bytes,
            started: now(),
            last_logged_ms: 0,
        })
    }

    fn dto(&self, ended: Option<i64>) -> PartDto {
        PartDto {
            path: self.path.to_string_lossy().into_owned(),
            stream_id: self.stream_id,
            bytes: self.bytes,
            started: self.started,
            ended,
        }
    }

    async fn log(&mut self, a: Arrival) {
        // A lost line costs a little accuracy in the clock, never the
        // recording, so a failed write is not an error.
        let _ = self.log.write_all(clock::log_line(&a).as_bytes()).await;
    }

    async fn write(&mut self, data: &[u8]) -> Result<()> {
        self.file
            .write_all(data)
            .await
            .context("writing the recording")?;
        self.bytes += data.len() as u64;
        let t = now_ms();
        // A few records a second is plenty for a clock that shows seconds.
        if t - self.last_logged_ms >= 200 {
            self.last_logged_ms = t;
            let a = Arrival {
                pos: self.bytes,
                wall_ms: t,
                connect: false,
            };
            self.log(a).await;
        }
        Ok(())
    }
}

/// Counts an open stream for as long as it lives.
struct StreamGuard<'a>(&'a Server);

impl<'a> StreamGuard<'a> {
    fn new(s: &'a Server) -> Self {
        s.streams_open.fetch_add(1, Ordering::SeqCst);
        Self(s)
    }
}

impl Drop for StreamGuard<'_> {
    fn drop(&mut self) {
        self.0.streams_open.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Whether the start of a body is an MPEG transport stream: a sync byte, and
/// another one packet later. A provider refusing politely sends a line of
/// text with a success status instead.
pub fn looks_like_ts(head: &[u8]) -> bool {
    (0..188.min(head.len()))
        .any(|i| head[i] == 0x47 && head.get(i + 188).is_none_or(|&b| b == 0x47))
}

/// A refusal's status and words, as a [`Failure`].
fn refusal(status: u16, body: &str) -> Failure {
    let mut clues = crate::ipc::ending::Clues::default();
    clues.observe(&format!(
        "HTTP error {status} {}",
        body.lines().next().unwrap_or("")
    ));
    Failure::Refused {
        status,
        limit: clues.limit,
    }
}

/// Opens the stream. A refusal is read for its words (a provider says "max
/// connections" in the body as often as in the status line).
async fn connect(p: &Playable) -> Result<reqwest::Response, Failure> {
    let http = crate::m3u::fetch::http_client(p.user_agent.as_deref())
        .map_err(|_| Failure::Unreachable)?;
    let mut req = http.get(&p.url);
    if let Some(r) = &p.referrer {
        req = req.header(reqwest::header::REFERER, r);
    }
    let resp = req.send().await.map_err(|_| Failure::Unreachable)?;
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = tokio::time::timeout(Duration::from_secs(3), resp.text())
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let body: String = body.chars().take(512).collect();
    Err(refusal(status.as_u16(), &body))
}

/// How a connection ended: a failure and how long it delivered, or `None`
/// when the recording is over (its end came, or it was stopped).
async fn pump(
    server: &Server,
    id: i64,
    resp: &mut reqwest::Response,
    part: &mut OpenPart,
    until: i64,
    stop: &mut watch::Receiver<bool>,
) -> Result<Option<(Failure, i64)>> {
    let connected = Instant::now();
    let mut first: Option<Instant> = None;
    let mut head: Vec<u8> = Vec::new();
    let mut checked = false;
    let delivered = |first: Option<Instant>| first.map_or(0, |f| f.elapsed().as_secs() as i64);
    loop {
        let left = until - now();
        if left <= 0 || *stop.borrow() {
            return Ok(None);
        }
        let deadline = Duration::from_secs(STALL_SECS as u64).min(Duration::from_secs(left as u64));
        let chunk = tokio::select! {
            r = tokio::time::timeout(deadline, resp.chunk()) => r,
            _ = stop.changed() => return Ok(None),
        };
        let chunk = match chunk {
            Err(_) if now() >= until => return Ok(None),
            Err(_) => return Ok(Some((Failure::Stalled, delivered(first)))),
            Ok(Err(_)) => return Ok(Some((Failure::Unreachable, delivered(first)))),
            Ok(Ok(None)) => {
                if !checked && !looks_like_ts(&head) {
                    return Ok(Some((Failure::NotVideo, 0)));
                }
                if !checked {
                    part.write(&head).await?;
                }
                return Ok(Some((Failure::Ended, delivered(first))));
            }
            Ok(Ok(Some(c))) => c,
        };
        if !checked {
            head.extend_from_slice(&chunk);
            if head.len() < 376 && connected.elapsed() < Duration::from_secs(STALL_SECS as u64) {
                continue;
            }
            if !looks_like_ts(&head) {
                return Ok(Some((Failure::NotVideo, 0)));
            }
            checked = true;
            first = Some(Instant::now());
            part.write(&std::mem::take(&mut head)).await?;
        } else {
            part.write(&chunk).await?;
        }
        let bytes = part.bytes;
        server.update_live(id, |l| l.bytes = bytes);
    }
}

/// Captures recording `id` until its window ends or it is stopped.
pub async fn run(server: &Arc<Server>, id: i64, mut stop: watch::Receiver<bool>) {
    if let Err(e) = run_inner(server, id, &mut stop).await {
        server.note(id, &format!("the capture stopped: {e:#}"));
    }
}

async fn run_inner(server: &Arc<Server>, id: i64, stop: &mut watch::Receiver<bool>) -> Result<()> {
    let Some(rec) = server.db(|c| store::get(c, id))? else {
        return Ok(());
    };
    let info = server.db(|c| store::capture_info(c, id))?;
    server.db(|c| store::set_state(c, id, RecordingState::Recording, None))?;
    let until = rec.stop + rec.pad_after;
    let folder = PathBuf::from(&rec.folder);
    let mut feeds = vec![rec.stream_id];
    if let Some(b) = rec.backup_stream_id.filter(|&b| b != rec.stream_id) {
        feeds.push(b);
    }
    let mut fallback = Fallback::new(feeds.len());
    let mut parts = rec.parts.clone();
    let mut open: Option<OpenPart> = None;
    let mut next_at = now();
    let mut slot_waits: u32 = 0;
    let mut last_failure: Option<Failure> = None;

    loop {
        if now() >= until || *stop.borrow() {
            break;
        }
        // Wait for the next attempt, the end, or a stop.
        let wait = (next_at.min(until) - now()).max(0) as u64;
        if wait > 0 {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
                _ = stop.changed() => break,
            }
            continue;
        }

        // Never take a connection the account does not have to spare.
        if let Slots::Full { active, max } = server.budget.check(server.open_streams()).await {
            slot_waits += 1;
            let why = format!("waiting for a free connection ({active} of {max} in use)");
            if slot_waits == 1 {
                server.note(id, &why);
            }
            server.update_live(id, |l| l.waiting = Some(why));
            next_at = now() + wait_for_slot(slot_waits, jitter());
            continue;
        }
        slot_waits = 0;
        server.update_live(id, |l| l.waiting = None);

        let stream_id = feeds[fallback.current()];
        let playable = match Source::load().and_then(|s| s.live(stream_id)) {
            Ok(p) => p,
            Err(e) => {
                server.note(id, &format!("cannot play channel {stream_id}: {e:#}"));
                let next = fallback.failed(now(), Failure::Unreachable, 0);
                next_at = next.at;
                continue;
            }
        };
        let guard = StreamGuard::new(server);
        let mut resp = match connect(&playable).await {
            Ok(r) => r,
            Err(f) => {
                drop(guard);
                last_failure = Some(f);
                let next = fallback.failed(now(), f, 0);
                if let Some(n) = next.note {
                    server.note(id, &n);
                }
                next_at = next.at;
                continue;
            }
        };

        // A new feed starts a new part; the same feed again continues its
        // part (the remuxer joins the timeline across the reconnect).
        if open.as_ref().is_none_or(|p| p.stream_id != stream_id) {
            if let Some(p) = open.take() {
                close(server, id, &mut parts, p)?;
            }
            let index = parts.len() + 1;
            let p = OpenPart::create(&folder, &info.base, index, stream_id).await?;
            parts.push(p.dto(None));
            server.db(|c| store::set_parts(c, id, &parts))?;
            server.update_live(id, |l| {
                l.stream_id = stream_id;
                l.since = p.started;
            });
            open = Some(p);
        }
        let part = open.as_mut().expect("a part was just opened");
        let pos = part.bytes;
        part.log(Arrival {
            pos,
            wall_ms: now_ms(),
            connect: true,
        })
        .await;

        let ended = pump(server, id, &mut resp, part, until, stop).await?;
        drop(resp);
        drop(guard);
        let Some((failure, delivered)) = ended else {
            break;
        };
        last_failure = Some(failure);
        let next = fallback.failed(now(), failure, delivered);
        if let Some(n) = next.note {
            server.note(id, &n);
        }
        next_at = next.at;
    }
    if let Some(p) = open.take() {
        close(server, id, &mut parts, p)?;
    }
    if parts.iter().all(|p| p.bytes == 0) {
        let why = last_failure.map_or("nothing was received".to_string(), |f| f.describe());
        server.db(|c| store::set_state(c, id, RecordingState::Recording, Some(&why)))?;
    }
    Ok(())
}

/// Records a part's final size and end.
fn close(server: &Server, id: i64, parts: &mut [PartDto], p: OpenPart) -> Result<()> {
    if let Some(slot) = parts
        .iter_mut()
        .rev()
        .find(|d| d.path == p.path.to_string_lossy())
    {
        *slot = p.dto(Some(now()));
    }
    server.db(|c| store::set_parts(c, id, parts))
}

/// Turns every captured part into its finished file, and the recording into
/// Done (or Failed, with nothing to show).
pub async fn finish(server: &Arc<Server>, id: i64) {
    let me = Arc::clone(server);
    let r = tokio::task::spawn_blocking(move || finish_blocking(&me, id)).await;
    match r {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            server.note(id, &format!("finishing failed: {e:#}"));
            let _ = server
                .db(|c| store::set_state(c, id, RecordingState::Failed, Some(&format!("{e:#}"))));
        }
        Err(e) => tracing::error!("recording {id}: the finishing thread failed: {e}"),
    }
}

/// Closes a recording the service stops in the middle of (lifecycle): the
/// parts captured so far are finished into their files as [`finish`] does,
/// so what was recorded plays, but while its window is open the recording
/// stays `recording`, and the next start of the service carries on in a new
/// part. Its window over, it is simply finished.
pub async fn suspend(server: &Arc<Server>, id: i64) {
    let me = Arc::clone(server);
    let r = tokio::task::spawn_blocking(move || -> Result<bool> {
        let Some(rec) = me.db(|c| store::get(c, id))? else {
            return Ok(false);
        };
        if now() >= rec.stop + rec.pad_after {
            finish_blocking(&me, id)?;
            return Ok(false);
        }
        let parts = remux_parts(&me, id, true)?;
        me.db(|c| store::set_parts(c, id, &parts))?;
        Ok(true)
    })
    .await;
    match r {
        Ok(Ok(true)) => server.note(
            id,
            "the recording service stopped: what was recorded so far is saved, and recording carries on in a new part if it starts again before the programme ends",
        ),
        Ok(Ok(false)) => {}
        Ok(Err(e)) => server.note(id, &format!("saving what was recorded failed: {e:#}")),
        Err(e) => tracing::error!("recording {id}: the finishing thread failed: {e}"),
    }
}

fn finish_blocking(server: &Server, id: i64) -> Result<()> {
    let done = remux_parts(server, id, false)?;
    server.db(|c| store::set_parts(c, id, &done))?;
    if done.is_empty() {
        let why = server
            .db(|c| store::get(c, id))?
            .and_then(|r| r.error)
            .unwrap_or_else(|| "no feed delivered anything".to_string());
        server.db(|c| store::set_state(c, id, RecordingState::Failed, Some(&why)))?;
    } else {
        server.db(|c| store::set_state(c, id, RecordingState::Done, None))?;
    }
    Ok(())
}

/// Remuxes each captured part not finished yet into its file and returns
/// every part. `more`: parts may follow (the recording is only suspended),
/// so even a lone part is named as a part (`… - part1.mkv`), as the next
/// will be.
fn remux_parts(server: &Server, id: i64, more: bool) -> Result<Vec<PartDto>> {
    let Some(rec) = server.db(|c| store::get(c, id))? else {
        return Ok(Vec::new());
    };
    let info = server.db(|c| store::capture_info(c, id))?;
    let folder = PathBuf::from(&rec.folder);
    let mut done: Vec<PartDto> = Vec::new();
    // Empty parts (a connection that delivered nothing) are dropped.
    let (keep, empty): (Vec<PartDto>, Vec<PartDto>) = rec.parts.into_iter().partition(|p| {
        let len = std::fs::metadata(&p.path).map(|m| m.len()).unwrap_or(0);
        len > 0
    });
    for p in empty {
        let _ = std::fs::remove_file(&p.path);
        let _ = std::fs::remove_file(log_path(Path::new(&p.path)));
    }
    let total = keep.len() + usize::from(more);
    for (i, mut part) in keep.into_iter().enumerate() {
        let src = PathBuf::from(&part.path);
        if src.extension().is_some_and(|e| e != "ts")
            || !src
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            // Finished already (by an earlier run).
            done.push(part);
            continue;
        }
        part.bytes = std::fs::metadata(&src)
            .map(|m| m.len())
            .unwrap_or(part.bytes);
        let name = naming::unique(&naming::file(&info.base, i + 1, total, "mkv"), |n| {
            folder.join(n).exists()
        });
        let dst = folder.join(&name);
        let tmp = folder.join(format!(".{name}.partial"));
        let log = log_path(&src);
        match make_mkv(&src, &log, &tmp, info.clock_track) {
            Ok(()) => {
                std::fs::rename(&tmp, &dst).with_context(|| format!("naming {}", dst.display()))?;
                let _ = std::fs::remove_file(&src);
                let _ = std::fs::remove_file(&log);
                part.path = dst.to_string_lossy().into_owned();
                part.bytes = std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                let ts = naming::unique(&naming::file(&info.base, i + 1, total, "ts"), |n| {
                    folder.join(n).exists()
                });
                let kept = folder.join(&ts);
                std::fs::rename(&src, &kept)
                    .with_context(|| format!("naming {}", kept.display()))?;
                let _ = std::fs::remove_file(&log);
                server.note(
                    id,
                    &format!("part {} could not be remuxed ({e:#}); kept as {ts}", i + 1),
                );
                part.path = kept.to_string_lossy().into_owned();
            }
        }
        done.push(part);
    }
    Ok(done)
}

/// Remuxes one part, with the clock track drawn from its arrival log.
fn make_mkv(ts: &Path, log: &Path, out: &Path, clock_track: bool) -> Result<()> {
    if !clock_track {
        return remux::remux(ts, out, None);
    }
    let arrivals = clock::parse_log(&std::fs::read_to_string(log).unwrap_or_default());
    let scan = remux::scan(ts)?;
    let cues = clock::cues(
        &scan.samples,
        &arrivals,
        scan.duration_ms,
        crate::localtime::utc_offset,
    );
    if cues.is_empty() {
        tracing::warn!(
            "{}: no arrival times; writing it without the clock track",
            ts.display()
        );
        return remux::remux(ts, out, None);
    }
    remux::remux(ts, out, Some((&clock::ass_header(), &cues)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transport_stream_is_told_from_a_line_of_text() {
        let mut ts = vec![0u8; 400];
        ts[3] = 0x47;
        ts[3 + 188] = 0x47;
        assert!(looks_like_ts(&ts));
        assert!(!looks_like_ts(b"Max connections reached\n"));
        assert!(!looks_like_ts(&[0u8; 400]));
    }

    #[test]
    fn a_refusal_that_speaks_of_a_limit_is_one() {
        assert_eq!(
            refusal(403, "Max Connections Reached"),
            Failure::Refused {
                status: 403,
                limit: true
            }
        );
        assert_eq!(
            refusal(403, "Forbidden"),
            Failure::Refused {
                status: 403,
                limit: false
            }
        );
    }
}
