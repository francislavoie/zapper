//! Capturing a recording: the stream's bytes, copied to disk as they come,
//! then each part remuxed to Matroska.
//!
//! The bytes come from the feed's [`super::hub`]: the service's one
//! connection to that feed, shared with any other recording of it at the
//! same time. They go to the part file untouched: nothing is decoded or
//! re-encoded. Beside each part a small log records when its bytes arrived
//! (see [`super::clock`]), which is what the clock track is drawn from.
//!
//! Which feed to ride, and when, is [`super::fallback`]'s decision. A hub
//! already open for one of the recording's feeds is ridden at no cost;
//! opening one asks the service's [`super::budget`] first, and on a full
//! account follows [`super::sharing`]'s rules.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;

use super::api::{PartDto, RecordingState, WhileRecording};
use super::budget::{jitter, wait_for_slot, Slots};
use super::clock::{self, Arrival};
use super::fallback::{Failure, Fallback};
use super::hub::{Subscription, SETTLE_LOOK_SECS};
use super::naming;
use super::server::Server;
use super::sharing::{self, Full, Rider};
use super::{now, now_ms, remux, store, strm};

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
    /// A new part file and its arrival log.
    async fn create(
        folder: &Path,
        base: &str,
        index: usize,
        stream_id: i64,
        visible: bool,
    ) -> Result<Self> {
        let (path, file) = create_capture_file(folder, base, index, visible).await?;
        let log = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(log_path(&path))
            .await?;
        Ok(Self {
            stream_id,
            path,
            file,
            log,
            bytes: 0,
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
            handed_over: false,
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

/// A new capture file for part `index`, under a name nothing else has: two
/// recordings of the same airing (on two channels) have the same name, and
/// must never write into one file.
async fn create_capture_file(
    folder: &Path,
    base: &str,
    index: usize,
    visible: bool,
) -> Result<(PathBuf, tokio::fs::File)> {
    tokio::fs::create_dir_all(folder)
        .await
        .with_context(|| format!("creating {}", folder.display()))?;
    let name = naming::capture_file(base, index, visible);
    let mut n = 1;
    loop {
        let path = folder.join(if n == 1 {
            name.clone()
        } else {
            naming::numbered(&name, n)
        });
        match tokio::fs::OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
            .await
        {
            Ok(f) => return Ok((path, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
        }
    }
}

/// Whether a connection of its own may be opened now.
enum Room {
    Go,
    /// Not now; look again at `at`. `note`: the first wait worth a note.
    Wait {
        at: i64,
        why: String,
        note: bool,
    },
}

/// Asks the budget, and on a full account applies the sharing rules for
/// `me`, recording `title`. `waits` counts the waits in a row, for the
/// back-off.
async fn room(server: &Arc<Server>, me: Rider, title: &str, waits: &mut u32) -> Room {
    let own = server.hubs.open_count();
    let Slots::Full { active, max } = server.budget.check(own).await else {
        return Room::Go;
    };
    // A browser watching through the service is never cut off (sharing,
    // rule 5); its page is told this recording waits for it.
    let watched = server
        .hubs
        .conns()
        .iter()
        .any(|c| c.riders.iter().any(|r| r.stop == i64::MAX));
    let why = format!(
        "waiting for a free connection ({active} of {max} in use{})",
        if watched { ", one by a browser watching" } else { "" }
    );
    let t = now();
    // A connection is being closed for a recording, or the provider counts
    // more than the service holds and one of the service's closed moments
    // ago (providers are slow to let go): look again shortly rather than
    // back off, or close another for the same need. A count the service
    // keeps itself (a playlist's setting) is never behind.
    if server.hubs.closing() || (active > own as i64 && server.hubs.released_lately()) {
        return Room::Wait {
            at: t + SETTLE_LOOK_SECS,
            why,
            note: false,
        };
    }
    let own = server.hubs.conns();
    match sharing::when_full(t, me.stop, &own) {
        Full::Cut(i) => {
            server.end_padding(own[i].feed, title, t);
            Room::Wait {
                at: t + SETTLE_LOOK_SECS,
                why,
                note: false,
            }
        }
        Full::Wait { until } => {
            *waits += 1;
            let mut at = t + wait_for_slot(*waits, jitter());
            if let Some(u) = until {
                at = at.min(u);
            }
            Room::Wait {
                at,
                why,
                note: *waits == 1,
            }
        }
    }
}

/// What the recording gets from its hub until its window ends, it is
/// stopped (both `None`), or the feed fails: how, and for how long the feed
/// had delivered.
async fn receive(
    server: &Server,
    id: i64,
    sub: &mut Subscription,
    part: &mut OpenPart,
    until: i64,
    stop: &mut watch::Receiver<bool>,
) -> Result<Option<(Failure, i64)>> {
    loop {
        let left = until - now();
        if left <= 0 || *stop.borrow() {
            return Ok(None);
        }
        let data = tokio::select! {
            d = sub.recv() => d,
            _ = tokio::time::sleep(Duration::from_secs(left as u64)) => continue,
            _ = stop.changed() => return Ok(None),
        };
        let Some(data) = data else {
            return Ok(Some(sub.ended().unwrap_or((Failure::Ended, 0))));
        };
        part.write(&data).await?;
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
    // For Jellyfin: a file pointing at the live stream of what is about to
    // be written (see `strm`). Without one the recording goes on all the
    // same.
    if server.config().while_recording == WhileRecording::Strm {
        if let Err(e) = strm::ensure(server, id) {
            server.note(id, &format!("could not write its .strm file: {e:#}"));
        }
    }
    let until = rec.stop + rec.pad_after;
    let me = Rider {
        id,
        stop: rec.stop,
        until,
    };
    let folder = PathBuf::from(&rec.folder);
    let mut feeds = vec![rec.stream_id];
    if let Some(b) = rec.backup_stream_id.filter(|&b| b != rec.stream_id) {
        feeds.push(b);
    }
    let mut fallback = Fallback::new(feeds.len());
    let mut open: Option<OpenPart> = None;
    let mut next_at = now();
    let mut slot_waits: u32 = 0;
    let mut waiting_for_room = false;
    let mut last_failure: Option<Failure> = None;
    let mut lost_noted = false;
    let mut changes = server.hubs.watch();

    loop {
        if now() >= until || *stop.borrow() {
            break;
        }
        // Wait for the next attempt, the end, or a stop. A hub opening on
        // one of its feeds, or one closing while it waits for room, is worth
        // looking at at once.
        let wait = (next_at.min(until) - now()).max(0) as u64;
        if wait > 0 {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
                _ = stop.changed() => break,
                r = changes.changed() => {
                    if r.is_ok()
                        && (waiting_for_room || feeds.iter().any(|&f| server.hubs.has(f)))
                    {
                        next_at = now();
                    }
                }
            }
            continue;
        }

        // The backup may have been chosen by hand since the last attempt
        // (the request that changed it left a note): it applies from this
        // one.
        let backup = server
            .db(|c| store::backup_of(c, id))?
            .filter(|&b| b != rec.stream_id);
        if backup != feeds.get(1).copied() {
            feeds.truncate(1);
            feeds.extend(backup);
            fallback.backup_changed(backup.is_some());
        }

        // Ride a connection already open for one of its feeds, the current
        // one first: that costs the account nothing.
        let current = fallback.current();
        let order: Vec<i64> = std::iter::once(feeds[current])
            .chain(feeds.iter().copied().filter(|&f| f != feeds[current]))
            .collect();
        let mut sub = match server.hubs.join(&order, me) {
            Some((i, sub)) => {
                if let Some(at) = feeds.iter().position(|&f| f == order[i]) {
                    fallback.ride(at);
                }
                sub
            }
            None => {
                // A connection of its own: never one the account does not
                // have to spare.
                if let Room::Wait { at, why, note } =
                    room(server, me, &rec.title, &mut slot_waits).await
                {
                    if note {
                        server.note(id, &why);
                    }
                    server.update_live(id, |l| l.waiting = Some(why));
                    waiting_for_room = true;
                    next_at = at;
                    continue;
                }
                let stream_id = feeds[fallback.current()];
                let playable = match server.playable(stream_id) {
                    Ok(p) => p,
                    Err(e) => {
                        server.note(id, &format!("cannot play channel {stream_id}: {e:#}"));
                        let next = fallback.failed(now(), Failure::Unreachable, 0);
                        next_at = next.at;
                        continue;
                    }
                };
                server.hubs.open(stream_id, playable, me)
            }
        };
        slot_waits = 0;
        waiting_for_room = false;
        server.update_live(id, |l| l.waiting = None);
        let stream_id = sub.feed();

        // A new feed starts a new part; the same feed again continues its
        // part (the remuxer joins the timeline across the reconnect).
        if open.as_ref().is_none_or(|p| p.stream_id != stream_id) {
            if let Some(p) = open.take() {
                close(server, id, p).await?;
            }
            // Numbered among what the service received: a part handed over
            // by the app is not one other programs see (`add_prelude`).
            let index = server
                .db(|c| store::get(c, id))?
                .map_or(0, |r| r.parts.iter().filter(|p| !p.handed_over).count())
                + 1;
            let visible = server.config().while_recording == WhileRecording::Visible;
            let p = OpenPart::create(&folder, &info.base, index, stream_id, visible).await?;
            let dto = p.dto(None);
            server.db(|c| store::edit_parts(c, id, |ps| ps.push(dto)))?;
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

        let ended = receive(server, id, &mut sub, part, until, stop).await?;
        let lost = sub.lost();
        drop(sub);
        if lost > 0 && !lost_noted {
            lost_noted = true;
            server.note(
                id,
                &format!(
                    "{} kB of the stream were dropped: writing to the disk did not keep up",
                    lost / 1000
                ),
            );
        }
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
        close(server, id, p).await?;
    }
    let parts = server.db(|c| store::get(c, id))?.map(|r| r.parts).unwrap_or_default();
    if parts.iter().all(|p| p.bytes == 0) {
        let why = last_failure.map_or("nothing was received".to_string(), |f| f.describe());
        server.db(|c| store::set_error(c, id, Some(&why)))?;
    }
    Ok(())
}

/// The most a part handed over by the app may weigh: far more than any
/// player's rewind buffer holds.
pub const PRELUDE_MAX: u64 = 8 << 30;

/// Takes what the app's player had buffered of the programme before the
/// recording began, as a part of its own ahead of what the capture receives
/// (docs/recording.md, "Recording what you watch"). `first_ms`: when its
/// first packet arrived, Unix milliseconds, which places it on the clock
/// track. `body` is MPEG-TS, as the player wrote it out of its cache.
///
/// Only while the recording is still to come or being captured: once it is
/// being saved into its files there is no part left to put it in front of.
pub async fn add_prelude<S, E>(
    server: &Arc<Server>,
    id: i64,
    first_ms: i64,
    mut body: S,
) -> Result<PartDto>
where
    S: futures_core::Stream<Item = std::result::Result<bytes::Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    let open = |r: &super::api::RecordingDto| {
        matches!(r.state, RecordingState::Scheduled | RecordingState::Recording)
    };
    let rec = server
        .db(|c| store::get(c, id))?
        .ok_or_else(|| anyhow::anyhow!("no such recording"))?;
    if !open(&rec) {
        anyhow::bail!("that recording is no longer being recorded");
    }
    let info = server.db(|c| store::capture_info(c, id))?;
    // Hidden whatever the setting: other programs following a recording
    // while it records (the growing file, or the `.strm`'s live stream) see
    // what the service receives, in the order it is received, and a part
    // put in front of it later would move everything they have read. It
    // joins in the finished files.
    let (path, mut file) =
        create_capture_file(Path::new(&rec.folder), &info.base, rec.parts.len() + 1, false)
            .await?;
    let written = async {
        let mut bytes = 0u64;
        let mut head: Vec<u8> = Vec::new();
        while let Some(chunk) =
            std::future::poll_fn(|cx| futures_core::Stream::poll_next(std::pin::Pin::new(&mut body), cx))
                .await
        {
            let chunk = chunk.map_err(|e| anyhow::anyhow!("the upload broke off: {e}"))?;
            if head.len() < 2 * 188 + 1 {
                head.extend_from_slice(&chunk[..chunk.len().min(400)]);
            }
            bytes += chunk.len() as u64;
            if bytes > PRELUDE_MAX {
                anyhow::bail!("that is more than a player buffers");
            }
            file.write_all(&chunk).await.context("writing the recording")?;
        }
        file.flush().await?;
        if super::hub::sync_at(&head) != Some(0) {
            anyhow::bail!("that is not an MPEG transport stream");
        }
        Ok::<u64, anyhow::Error>(bytes)
    }
    .await;
    drop(file);
    let discard = |p: &Path| {
        let _ = std::fs::remove_file(p);
        let _ = std::fs::remove_file(log_path(p));
    };
    let bytes = match written {
        Ok(b) => b,
        Err(e) => {
            discard(&path);
            return Err(e);
        }
    };
    // Read for its timeline now, so a file the remuxer cannot read is
    // refused while the app can still say so, and the clock has its log.
    let scanned = {
        let p = path.clone();
        tokio::task::spawn_blocking(move || remux::scan(&p))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
    };
    let scan = match scanned {
        Ok(s) if !s.samples.is_empty() => s,
        Ok(_) => {
            discard(&path);
            anyhow::bail!("there is no picture or sound in it");
        }
        Err(e) => {
            discard(&path);
            return Err(e.context("it cannot be read"));
        }
    };
    let log: String = clock::replayed(&scan.samples, first_ms)
        .iter()
        .map(clock::log_line)
        .collect();
    tokio::fs::write(log_path(&path), log).await?;
    let started = first_ms.div_euclid(1000);
    let part = PartDto {
        path: path.to_string_lossy().into_owned(),
        stream_id: rec.stream_id,
        bytes,
        started,
        ended: Some(started + scan.duration_ms / 1000),
        handed_over: true,
    };
    // In order of when each began: ahead of what the capture has received
    // since, and behind anything from before it (a service that stopped and
    // started again in the middle).
    let added = server.db(|c| {
        let Some(now_rec) = store::get(c, id)? else {
            return Ok(false);
        };
        if !open(&now_rec) {
            return Ok(false);
        }
        let p = part.clone();
        store::edit_parts(c, id, move |parts| {
            let at = parts.partition_point(|x| x.started <= p.started);
            parts.insert(at, p);
        })?;
        Ok(true)
    })?;
    if !added {
        discard(&path);
        anyhow::bail!("that recording is no longer being recorded");
    }
    server.note(
        id,
        &format!(
            "the first {} came from what this computer's player had already received",
            length(scan.duration_ms / 1000)
        ),
    );
    Ok(part)
}

/// "40 s", "3 min 20 s": a length for a note.
fn length(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => format!("{s} s"),
        s if s % 60 == 0 => format!("{} min", s / 60),
        s => format!("{} min {} s", s / 60, s % 60),
    }
}

/// Records a part's final size and end, once all of it is in the file: a
/// reader following the part (`strm`) takes "ended" to mean nothing more
/// will come.
async fn close(server: &Server, id: i64, mut p: OpenPart) -> Result<()> {
    let _ = p.file.flush().await;
    let done = p.dto(Some(now()));
    server.db(|c| {
        store::edit_parts(c, id, |parts| {
            if let Some(slot) = parts.iter_mut().rev().find(|d| d.path == done.path) {
                *slot = done;
            }
        })
    })
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
            strm::finished(server, id);
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
    // Its `.mkv` is in place (or it failed): the `.strm` that stood in for
    // it goes.
    strm::finished(server, id);
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
        if !src
            .file_name()
            .is_some_and(|n| naming::is_capture_file(&n.to_string_lossy(), &info.base))
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
