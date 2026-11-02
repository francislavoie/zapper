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

use super::api::{PartDto, RecordingState};
use super::budget::{jitter, wait_for_slot, Slots};
use super::clock::{self, Arrival};
use super::fallback::{Failure, Fallback};
use super::hub::{Subscription, SETTLE_LOOK_SECS};
use super::naming;
use super::server::Server;
use super::sharing::{self, Full, Rider};
use super::{now, now_ms, remux, store};
use crate::source::Source;

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
    /// A new part file, under a name nothing else has: two recordings of
    /// the same airing (on two channels) have the same name, and must never
    /// write into one file.
    async fn create(
        folder: &Path,
        base: &str,
        index: usize,
        stream_id: i64,
        visible: bool,
    ) -> Result<Self> {
        tokio::fs::create_dir_all(folder)
            .await
            .with_context(|| format!("creating {}", folder.display()))?;
        let name = naming::capture_file(base, index, visible);
        let mut n = 1;
        let (path, file) = loop {
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
                Ok(f) => break (path, f),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
                Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
            }
        };
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
    let why = format!("waiting for a free connection ({active} of {max} in use)");
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
    let mut parts = rec.parts.clone();
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
                let playable = match Source::load().and_then(|s| s.live(stream_id)) {
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
                close(server, id, &mut parts, p)?;
            }
            let index = parts.len() + 1;
            let visible = server.config().show_while_recording;
            let p = OpenPart::create(&folder, &info.base, index, stream_id, visible).await?;
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
