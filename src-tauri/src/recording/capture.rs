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
//!
//! **Off air.** A channel that is not broadcasting may be answered with a
//! placeholder clip (`hub`, "Off air"). Each connection's first data is
//! held back until the hub has told it from a clip ([`receive`]), which
//! keeps a clip out of the file (the limits are `receive`'s and
//! [`HOLD_MAX`]'s; what is held is in memory, so a crash loses it). The
//! recording notes "Off air at 18:26; retrying" once, and asks again until
//! its programme ends: each feed every [`OFF_AIR_RETRY_SECS`], taking turns
//! with a backup (`fallback`). A try rides any connection the service has
//! to either feed, and opens one only when one is free outright
//! ([`free_now`]): never through the sharing rules, so it closes nobody's
//! padding, and its wait is a retry time (`Live::retry_at`), never
//! "waiting for a free connection", which would tell a browser or the app
//! that it needs theirs.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;

use super::api::{PartDto, RecordingState, WhileRecording};
use super::budget::{jitter, wait_for_slot, Slots};
use super::clock::{self, Arrival};
use super::fallback::{Failure, Fallback, OFF_AIR_RETRY_SECS};
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
    /// When the last connection that wrote into it ended (Unix seconds):
    /// the part's end when it is closed later, which may be after another
    /// connection's data was held back for a while.
    last_end: Option<i64>,
    last_logged_ms: i64,
}

/// Held data is written in pieces of about this much, not one write per
/// chunk it arrived in.
const COALESCE: usize = 1 << 20;

impl OpenPart {
    /// A new part file and its arrival log. `started`: when its first data
    /// arrived (Unix seconds), which is not when it is written for data that
    /// was held back.
    async fn create(
        folder: &Path,
        base: &str,
        index: usize,
        stream_id: i64,
        visible: bool,
        started: i64,
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
            started,
            last_end: None,
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

    /// Writes `data`, which arrived at `t` (Unix milliseconds).
    async fn write(&mut self, data: &[u8], t: i64) -> Result<()> {
        self.file
            .write_all(data)
            .await
            .context("writing the recording")?;
        self.bytes += data.len() as u64;
        self.arrived(self.bytes, t).await;
        Ok(())
    }

    /// Writes pieces that arrived at different times (Unix milliseconds,
    /// each with its piece), joined into writes of about [`COALESCE`]; the
    /// arrival log is as if each had been written as it came.
    async fn write_pieces(&mut self, pieces: &[(i64, bytes::Bytes)]) -> Result<()> {
        let mut buf: Vec<u8> = Vec::new();
        let mut ends: Vec<(u64, i64)> = Vec::new();
        for (i, (t, data)) in pieces.iter().enumerate() {
            buf.extend_from_slice(data);
            ends.push((self.bytes + buf.len() as u64, *t));
            if buf.len() >= COALESCE || i + 1 == pieces.len() {
                self.file
                    .write_all(&buf)
                    .await
                    .context("writing the recording")?;
                self.bytes += buf.len() as u64;
                buf.clear();
                for (pos, t) in ends.drain(..) {
                    self.arrived(pos, t).await;
                }
            }
        }
        Ok(())
    }

    /// The arrival log's record that the file was `pos` bytes long at `t`.
    /// A few records a second is plenty for a clock that shows seconds.
    async fn arrived(&mut self, pos: u64, t: i64) {
        if t - self.last_logged_ms >= 200 {
            self.last_logged_ms = t;
            let a = Arrival {
                pos,
                wall_ms: t,
                connect: false,
            };
            self.log(a).await;
        }
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
    // Movies browsers watch included: each holds a connection of its own.
    let own = server.open_streams();
    let Slots::Full { active, max } = server.budget.check(own).await else {
        return Room::Go;
    };
    // A browser watching through the service (a channel or a movie) is
    // never cut off (sharing, rule 5); its page is told this recording
    // waits for it.
    let watched = server
        .own_conns()
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
    let released = server.hubs.released_lately() || server.vod.released_recently() > 0;
    if server.hubs.closing() || (active > own as i64 && released) {
        return Room::Wait {
            at: t + SETTLE_LOOK_SECS,
            why,
            note: false,
        };
    }
    let own = server.own_conns();
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

/// Whether a recording off air may open a connection of its own for a try.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OffAirRoom {
    /// The account has one free: try.
    Free,
    /// One of the service's connections is being closed, or was let go too
    /// lately for the provider to have stopped counting it: look again in
    /// a moment ([`SETTLE_LOOK_SECS`]), as the release settles. Most often
    /// the recording's own last try, just before it turns to its backup.
    Settling,
    /// The account is full: this try is skipped.
    Full,
}

/// The sharing rules ([`room`]) are not asked: an off-air try never closes
/// another recording's padding or waits its turn for a connection. An
/// account whose count nobody can tell (`Slots::Unknown`) is taken as
/// free, as any start takes it.
async fn free_now(server: &Server) -> OffAirRoom {
    if server.hubs.closing()
        || server.hubs.released_lately()
        || server.vod.released_recently() > 0
    {
        return OffAirRoom::Settling;
    }
    if server.budget.check(server.open_streams()).await.is_full() {
        OffAirRoom::Full
    } else {
        OffAirRoom::Free
    }
}

/// The most a recording holds back of a connection's first data while its
/// hub cannot yet tell it from a placeholder clip (`hub`, "Off air"). The
/// clips seen are a few megabytes for ten minutes of black; a broadcast's
/// connect burst at a high bitrate can come to more than this in the
/// twenty seconds, and past it what is held is written, the rest as it
/// comes.
pub const HOLD_MAX: usize = 64 << 20;

/// Where one connection's data goes: the recording's part for its feed,
/// opened at the first data written. So a connection that brings nothing,
/// or only a placeholder clip, leaves nothing behind: no part, no byte, no
/// line in the arrival log.
struct Sink<'a> {
    server: &'a Server,
    id: i64,
    folder: &'a Path,
    base: &'a str,
    stream_id: i64,
    open: &'a mut Option<OpenPart>,
    /// When the connection was made, Unix milliseconds: the arrival log's
    /// reconnect line.
    connect_ms: i64,
    /// Something was written.
    wrote: bool,
}

impl Sink<'_> {
    async fn write(&mut self, data: &[u8], at_ms: i64) -> Result<()> {
        let part = self.part().await?;
        part.write(data, at_ms).await?;
        let bytes = part.bytes;
        self.server.update_live(self.id, |l| l.bytes = bytes);
        Ok(())
    }

    /// What was held back, in a few large writes.
    async fn write_held(&mut self, held: Held) -> Result<()> {
        if held.chunks.is_empty() {
            return Ok(());
        }
        let part = self.part().await?;
        part.write_pieces(&held.chunks).await?;
        let bytes = part.bytes;
        self.server.update_live(self.id, |l| l.bytes = bytes);
        Ok(())
    }

    /// The part to write into, begun at the first write.
    async fn part(&mut self) -> Result<&mut OpenPart> {
        if !self.wrote {
            self.begin().await?;
            self.wrote = true;
        }
        Ok(self.open.as_mut().expect("a part was opened"))
    }

    /// A new feed starts a new part; the same feed again continues its part
    /// (the remuxer joins the timeline across the reconnect).
    async fn begin(&mut self) -> Result<()> {
        let (server, id, stream_id) = (self.server, self.id, self.stream_id);
        if self.open.as_ref().is_none_or(|p| p.stream_id != stream_id) {
            if let Some(p) = self.open.take() {
                close(server, id, p).await?;
            }
            // Numbered among what the service received: a part handed over
            // by the app is not one other programs see (`add_prelude`).
            let index = server
                .db(|c| store::get(c, id))?
                .map_or(0, |r| r.parts.iter().filter(|p| !p.handed_over).count())
                + 1;
            let visible = server.config().while_recording == WhileRecording::Visible;
            // Begun when its connection was made, not when what was held
            // back from it is written.
            let started = self.connect_ms.div_euclid(1000);
            let p = OpenPart::create(self.folder, self.base, index, stream_id, visible, started)
                .await?;
            let dto = p.dto(None);
            server.db(|c| store::edit_parts(c, id, |ps| ps.push(dto)))?;
            server.update_live(id, |l| {
                l.stream_id = stream_id;
                l.since = p.started;
            });
            *self.open = Some(p);
        }
        let part = self.open.as_mut().expect("a part was just opened");
        let pos = part.bytes;
        part.log(Arrival {
            pos,
            wall_ms: self.connect_ms,
            connect: true,
        })
        .await;
        Ok(())
    }
}

/// What a connection delivered before its hub could tell it from a
/// placeholder clip, with when each piece arrived (for the clock).
#[derive(Default)]
struct Held {
    chunks: Vec<(i64, bytes::Bytes)>,
    bytes: usize,
}

impl Held {
    async fn flush(self, sink: &mut Sink<'_>) -> Result<()> {
        sink.write_held(self).await
    }
}

/// What the recording gets from its hub until its window ends, it is
/// stopped (both `None`), or the feed fails: how, and for how long the feed
/// had delivered.
///
/// Until the hub has judged the stream not to be a placeholder clip (its
/// first `OFF_AIR_WINDOW`, see `hub`), what comes is held back: a clip ends
/// within it, and is then dropped whole, so a channel that is off air never
/// adds to the recording, however often it is asked again. Anything else
/// that ends the stream, or the recording, writes what was held.
async fn receive(
    sink: &mut Sink<'_>,
    sub: &mut Subscription,
    until: i64,
    stop: &mut watch::Receiver<bool>,
) -> Result<Option<(Failure, i64)>> {
    let mut judged = sub.judged();
    let mut held = (!*judged.borrow_and_update()).then(Held::default);
    loop {
        let left = until - now();
        if left <= 0 || *stop.borrow() {
            if let Some(h) = held.take() {
                h.flush(sink).await?;
            }
            return Ok(None);
        }
        let data = tokio::select! {
            d = sub.recv() => d,
            _ = tokio::time::sleep(Duration::from_secs(left as u64)) => continue,
            _ = stop.changed() => {
                if let Some(h) = held.take() {
                    h.flush(sink).await?;
                }
                return Ok(None);
            }
            r = judged.changed(), if held.is_some() => {
                // The hub only ever sets it to true. `changed` cannot fail
                // while `sub` holds the hub, whose sender it is; if it ever
                // did, nothing would judge the stream any more, so what was
                // held is written rather than waited on (and this branch,
                // with nothing held, is not polled again).
                if r.is_err() || *judged.borrow_and_update() {
                    if let Some(h) = held.take() {
                        h.flush(sink).await?;
                    }
                }
                continue;
            }
        };
        let Some(data) = data else {
            let ended = sub.ended().unwrap_or((Failure::Ended, 0));
            if let Some(h) = held.take() {
                if !matches!(ended.0, Failure::OffAir { .. }) {
                    h.flush(sink).await?;
                }
            }
            return Ok(Some(ended));
        };
        match held.as_mut() {
            Some(h) => {
                h.bytes += data.len();
                h.chunks.push((now_ms(), data));
                if h.bytes > HOLD_MAX {
                    if let Some(h) = held.take() {
                        h.flush(sink).await?;
                    }
                }
            }
            None => sink.write(&data, now_ms()).await?,
        }
    }
}

/// How long a recording whose channel is off air waits before asking again:
/// [`OFF_AIR_RETRY_SECS`], or for a harness build (and the unit tests),
/// `ZAPPER_OFF_AIR_RETRY_SECS` (a check cannot wait minutes for each try).
fn off_air_retry() -> i64 {
    #[cfg(any(test, feature = "harness"))]
    if let Some(s) = std::env::var("ZAPPER_OFF_AIR_RETRY_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
    {
        return s;
    }
    OFF_AIR_RETRY_SECS
}

/// `18:26`, local time: when, for a note.
fn clock_time(unix: i64) -> String {
    let c = crate::localtime::Civil::local(unix);
    format!("{:02}:{:02}", c.hour, c.minute)
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
    let mut fallback = Fallback::new(feeds.len()).with_off_air_retry(off_air_retry());
    let mut open: Option<OpenPart> = None;
    // When the channel was found off air, until a connection writes again:
    // one note for the spell, however often it is asked.
    let mut off_air_since: Option<i64> = None;
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
                // have to spare. Off air, only one that is free outright:
                // a try that may well find the clip again never waits its
                // turn, closes another recording's padding, or shows as
                // needing a connection. Without one, this try is skipped.
                if off_air_since.is_some() {
                    let room = free_now(server).await;
                    if room == OffAirRoom::Settling {
                        if now() + SETTLE_LOOK_SECS >= rec.stop {
                            break;
                        }
                        next_at = now() + SETTLE_LOOK_SECS;
                        server.update_live(id, |l| l.retry_at = Some(next_at));
                        continue;
                    }
                    if room == OffAirRoom::Full {
                        let t = now();
                        let next = fallback.skipped(t);
                        if let Some(n) = next.note {
                            server.note(id, &n);
                        }
                        if next.at >= rec.stop {
                            break;
                        }
                        next_at = next.at;
                        server.update_live(id, |l| l.retry_at = Some(next.at));
                        continue;
                    }
                } else if let Room::Wait { at, why, note } =
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
        server.update_live(id, |l| {
            l.waiting = None;
            l.retry_at = None;
        });

        let mut sink = Sink {
            server,
            id,
            folder: &folder,
            base: &info.base,
            stream_id: sub.feed(),
            open: &mut open,
            connect_ms: now_ms(),
            wrote: false,
        };
        let ended = receive(&mut sink, &mut sub, until, stop).await?;
        let wrote = sink.wrote;
        let lost = sub.lost();
        drop(sub);
        let off_air = matches!(ended, Some((Failure::OffAir { .. }, _)));
        // A clip comes faster than anything reads it; what it lost is not
        // the disk's doing.
        if lost > 0 && !lost_noted && !off_air {
            lost_noted = true;
            server.note(
                id,
                &format!(
                    "{} kB of the stream were dropped: writing to the disk did not keep up",
                    lost / 1000
                ),
            );
        }
        if wrote {
            off_air_since = None;
            // The part's end, should it be closed only after the next
            // connection's data was held back for a while.
            if let Some(p) = open.as_mut() {
                p.last_end = Some(now());
            }
        }
        let Some((failure, delivered)) = ended else {
            break;
        };
        last_failure = Some(failure);
        let t = now();
        let next = fallback.failed(t, failure, delivered);
        if let Some(n) = next.note {
            server.note(id, &n);
        }
        next_at = next.at;
        // Off air, it is asked again until the programme's end, never into
        // the padding after it: a connection then is only padding, which
        // the next recording's start may need.
        let again = next.at < rec.stop;
        if off_air && off_air_since.is_none() {
            off_air_since = Some(t);
            let retrying = if again { "; retrying" } else { "" };
            server.note(id, &format!("Off air at {}{retrying}", clock_time(t)));
        }
        if off_air_since.is_some() {
            if !again {
                break;
            }
            // Shown as a retry, never as waiting for a connection: it is
            // in nobody's way (`watch::notice`, the app's handover).
            server.update_live(id, |l| l.retry_at = Some(next.at));
        }
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
    let done = p.dto(Some(p.last_end.unwrap_or_else(now)));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::budget::Budget;
    use crate::recording::config::{self, ServerConfig};
    use std::sync::Mutex;

    /// A full account that notes how many streams the service said it held.
    struct Noting(Arc<Mutex<Vec<usize>>>);
    impl Budget for Noting {
        fn check(
            &self,
            own: usize,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Slots> + Send + '_>> {
            self.0.lock().unwrap().push(own);
            Box::pin(async { Slots::Full { active: 2, max: 2 } })
        }
    }

    #[tokio::test]
    async fn a_recording_asks_the_budget_counting_the_movies_browsers_watch() {
        let d = std::env::temp_dir().join(format!("zapper-capture-{}-budget", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("server.json");
        config::save_to(&path, &ServerConfig::default()).unwrap();
        let said = Arc::new(Mutex::new(Vec::new()));
        let s = Server::new(
            store::open_in_memory().unwrap(),
            ServerConfig::default(),
            path,
            "token".into(),
            Box::new(Noting(Arc::clone(&said))),
        );
        // A recording on channel 5, its feed slow to answer, and a browser
        // watching movie 9.
        let feed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let t = now();
        let _rec = s.hubs.open(
            5,
            crate::source::Playable::plain(format!("http://{}/5.ts", feed.local_addr().unwrap())),
            Rider {
                id: 1,
                stop: t + 3600,
                until: t + 3900,
            },
        );
        s.vod.add_for_test("device:a", 9, -1);
        assert_eq!(s.own_conns().len(), 2);

        let me = Rider {
            id: 2,
            stop: t + 3600,
            until: t + 3900,
        };
        let r = room(&s, me, "Evening", &mut 0).await;
        assert!(matches!(r, Room::Wait { .. }));
        assert_eq!(*said.lock().unwrap(), vec![2]);
        let _ = std::fs::remove_dir_all(&d);
    }

    // ------------------------------------------------------------ off air

    use crate::recording::api::ScheduleRequest;
    use crate::recording::hub::tests::{clip_server, stream_of, stream_server};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// An account that is full while the flag is set, and free otherwise;
    /// counts the times it was asked while full.
    struct Switch(Arc<AtomicBool>, Arc<AtomicUsize>);
    impl Budget for Switch {
        fn check(
            &self,
            _own: usize,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Slots> + Send + '_>> {
            let full = self.0.load(Ordering::SeqCst);
            if full {
                self.1.fetch_add(1, Ordering::SeqCst);
            }
            Box::pin(async move {
                if full {
                    Slots::Full { active: 1, max: 1 }
                } else {
                    Slots::Free
                }
            })
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zapper-capture-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn server_in(d: &Path, budget: Box<dyn Budget>) -> Arc<Server> {
        let path = d.join("server.json");
        config::save_to(&path, &ServerConfig::default()).unwrap();
        Server::new(
            store::open_in_memory().unwrap(),
            ServerConfig::default(),
            path,
            "token".into(),
            budget,
        )
    }

    /// A recording of `stream_id` from now for `secs`, no padding, its
    /// files in `folder`.
    fn recording(s: &Server, stream_id: i64, secs: i64, folder: &Path) -> i64 {
        let t = now();
        let req = ScheduleRequest {
            stream_id,
            backup_stream_id: None,
            channel_name: Some("Five".into()),
            title: "Night Show".into(),
            episode_title: None,
            season: None,
            episode: None,
            start: t,
            stop: t + secs,
            pad_before: Some(0),
            pad_after: Some(0),
        };
        let folder = folder.to_string_lossy().into_owned();
        s.db(|c| {
            store::insert(
                c,
                &store::NewRecording {
                    req: &req,
                    backup_stream_id: None,
                    channel_name: "Five",
                    pad_before: 0,
                    pad_after: 0,
                    folder: &folder,
                    base: "Night Show",
                    clock_track: true,
                    now: t,
                },
            )
        })
        .unwrap()
    }

    #[tokio::test]
    async fn an_off_air_try_opens_a_connection_only_when_one_is_free() {
        let d = scratch("free-now");
        let full = Arc::new(AtomicBool::new(true));
        let s = server_in(&d, Box::new(Switch(Arc::clone(&full), Arc::default())));
        assert_eq!(free_now(&s).await, OffAirRoom::Full, "a full account: skipped");
        full.store(false, Ordering::SeqCst);
        assert_eq!(free_now(&s).await, OffAirRoom::Free, "a free one: asked");
        // One of the service's own connections let go a moment ago: the
        // provider may still count it.
        let (url, _) = stream_server().await;
        let sub = s.hubs.open(
            8,
            crate::source::Playable::plain(url),
            Rider { id: 1, stop: now() + 600, until: now() + 600 },
        );
        drop(sub);
        let t = std::time::Instant::now();
        while s.hubs.open_count() > 0 {
            assert!(t.elapsed() < Duration::from_secs(5));
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(free_now(&s).await, OffAirRoom::Settling, "a release still settling: looked at again");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn a_recording_off_air_asks_again_without_closing_padding_or_waiting_for_a_connection() {
        // A harness's retry, so the test sees several tries.
        std::env::set_var("ZAPPER_OFF_AIR_RETRY_SECS", "2");
        let d = scratch("off-air-loop");
        let full = Arc::new(AtomicBool::new(false));
        let asked_full = Arc::new(AtomicUsize::new(0));
        let s = server_in(&d, Box::new(Switch(Arc::clone(&full), Arc::clone(&asked_full))));
        let (clip, clips) = clip_server(stream_of(0.0, 600.0)).await;
        let (live, _) = stream_server().await;
        let other = live.clone();
        s.set_playables(Arc::new(move |id| {
            Ok(crate::source::Playable::plain(if id == 5 { clip.clone() } else { live.clone() }))
        }));
        let id = recording(&s, 5, 300, &d);
        let (tx, stop) = watch::channel(false);
        s.live(|m| {
            m.insert(
                id,
                crate::recording::server::Live {
                    title: "Night Show".into(),
                    stream_id: 5,
                    bytes: 0,
                    since: now(),
                    waiting: None,
                    retry_at: None,
                    until: now() + 300,
                    finishing: false,
                    stop: tx.clone(),
                },
            )
        });
        let task = tokio::spawn({
            let s = Arc::clone(&s);
            async move { run(&s, id, stop).await }
        });
        let notes = |s: &Server| s.db(|c| store::get(c, id)).unwrap().unwrap().notes;
        // The first try is an ordinary start, and finds the clip.
        let t = std::time::Instant::now();
        while !notes(&s).iter().any(|n| n.starts_with("Off air at ")) {
            assert!(t.elapsed() < Duration::from_secs(10), "{:?}", notes(&s));
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(clips.load(Ordering::SeqCst), 1);

        // Then the account is full, its one connection carrying a recording
        // in its padding after: an ordinary start would close it (sharing,
        // rule 3). An off-air try does not, nor waits for a connection.
        full.store(true, Ordering::SeqCst);
        let padding = s.hubs.open(
            6,
            crate::source::Playable::plain(other),
            Rider { id: 99, stop: now() - 60, until: now() + 600 },
        );
        // Its own last try's connection settles first (it looks again every
        // few seconds meanwhile), then the full account skips its tries.
        let t = std::time::Instant::now();
        while asked_full.load(Ordering::SeqCst) < 2 {
            assert!(t.elapsed() < Duration::from_secs(45), "never asked the account");
            assert!(s.hubs.has(6), "the padding was closed");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        assert!(s.hubs.has(6) && !s.hubs.closing(), "the padding was left alone");
        assert_eq!(clips.load(Ordering::SeqCst), 1, "no try without a free connection");
        let said = notes(&s);
        assert_eq!(said.len(), 1, "one note for the spell: {said:?}");
        let (waiting, retry_at) = s.live(|m| m.get(&id).map(|l| (l.waiting.clone(), l.retry_at))).unwrap();
        assert_eq!(waiting, None, "never shown as needing a connection");
        assert!(retry_at.is_some_and(|r| r > now() - 1), "{retry_at:?}");
        let rec = s.db(|c| store::get(c, id)).unwrap().unwrap();
        assert!(rec.parts.is_empty(), "no part for a clip: {:?}", rec.parts);

        tx.send_replace(true);
        let _ = tokio::time::timeout(Duration::from_secs(10), task).await;
        drop(padding);
        std::env::remove_var("ZAPPER_OFF_AIR_RETRY_SECS");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn a_part_begins_when_its_connection_did_and_ends_when_it_did() {
        let d = scratch("part-times");
        let s = server_in(&d, Box::new(Switch(Arc::default(), Arc::default())));
        let id = recording(&s, 5, 300, &d);
        let connect_ms = now_ms() - 30_000;
        let mut open: Option<OpenPart> = None;
        let mut sink = Sink {
            server: &s,
            id,
            folder: &d,
            base: "Night Show",
            stream_id: 5,
            open: &mut open,
            connect_ms,
            wrote: false,
        };
        // Twenty seconds held back, then written at once.
        let piece = bytes::Bytes::from(vec![0x47u8; 188 * 4000]);
        let held = Held {
            chunks: (0..20).map(|i| (connect_ms + i * 1000, piece.clone())).collect(),
            bytes: piece.len() * 20,
        };
        held.flush(&mut sink).await.unwrap();
        let part = open.as_mut().unwrap();
        assert_eq!(part.bytes, (piece.len() * 20) as u64);
        let started = s.db(|c| store::get(c, id)).unwrap().unwrap().parts[0].started;
        assert_eq!(started, connect_ms.div_euclid(1000), "when the connection was made");
        // The arrival log has each piece at its own time, not the write's.
        let _ = part.log.flush().await;
        let log = clock::parse_log(&std::fs::read_to_string(log_path(&part.path)).unwrap());
        assert_eq!(log[0].wall_ms, connect_ms);
        assert!(log[0].connect);
        assert_eq!(log.last().unwrap().wall_ms, connect_ms + 19_000);
        assert_eq!(log.last().unwrap().pos, part.bytes);

        // Its connection ended long before it is closed (another feed's
        // data held back meanwhile): its end is the connection's.
        let ended_at = now() - 25;
        part.last_end = Some(ended_at);
        close(&s, id, open.take().unwrap()).await.unwrap();
        let ended = s.db(|c| store::get(c, id)).unwrap().unwrap().parts[0].ended;
        assert_eq!(ended, Some(ended_at));
        let _ = std::fs::remove_dir_all(&d);
    }
}
