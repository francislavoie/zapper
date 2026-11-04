//! Watching live TV in a browser signed in to the service
//! (docs/recording.md, "Watching in the browser").
//!
//! The browser never sees a provider address: it asks to watch a channel by
//! its id (`POST /api/watch`), is given a viewer of its own, and reads that
//! viewer's stream from here (`GET /api/stream/{viewer}`), as MPEG-TS for
//! the page's player.
//!
//! **Connections.** A viewer is one more subscriber of the service's
//! connection to the channel ([`super::hub`]), a rider whose programme never
//! ends ([`Rider::viewer`]), so the sharing rules never close it:
//!
//! - when the service already has the channel open (it is recording it,
//!   or another browser watches it), the viewer joins that connection and
//!   costs the account nothing;
//! - otherwise it needs a connection of its own, asked of the same budget
//!   the recordings ask ([`decide`]). On a full account it is refused with
//!   the reason, and "Watch here anyway" is offered only where that cannot
//!   cost a recording anything: it takes the place of another browser's
//!   viewer on this service, or, when the service holds nothing, connects
//!   as the app's player would (the provider settles it with whichever
//!   other device it ends). A recording's connection is never taken.
//! - A viewer that leaves gives its place up; the connection closes when
//!   nobody is left on it.
//!
//! **Recordings to come.** A viewer is never cut off for a recording, so on
//! a full account a recording due on another channel would wait for it. The
//! viewer is told instead ([`notice`]): from two minutes before the
//! recording's padded start, and for as long as it waits, the page shows
//! which recording needs the connection and offers to switch to its channel
//! (which shares one connection) or stop.
//!
//! **What the browser can play** is read from the stream itself
//! ([`tsprobe`]): the first packets go through [`tsprobe::Tables`] before
//! the answer, a channel whose video no browser plays (MPEG-2) is refused
//! with that reason, and what was learnt is kept per channel for a while, so
//! asking again costs no connection. Audio the browser lacks is encoded to
//! AAC on the way ([`livemux`]); the video never is.
//!
//! **The app's player** watches through here too, with the app's token and
//! `player` set, so its stream is passed on as it comes, every track in it,
//! and nothing is refused for its codecs (`super::handover` says when): when it
//! tunes to a channel the service is recording (`onlyShared`: a place on a
//! connection the service has, never a new one), and when Record is pressed
//! on the channel it plays on an account with no connection to spare, after
//! it has let its own connection go.
//!
//! **Movies** are watched through [`super::vodwatch`], on connections of
//! their own that count among the service's ([`Server::own_conns`]): the
//! budget, "anyway" and the limits below are the same for both.
//!
//! **Limits.** A device may hold [`PER_DEVICE`] viewers and start
//! [`STARTS_PER_MINUTE`] a minute; the service holds [`MAX_VIEWERS`] in
//! all, channels and movies together. A viewer nobody reads from for
//! [`IDLE`] is let go.

use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch, Notify};

use super::access::Caller;
use super::api::RecordingState;
use super::budget::Slots;
use super::fallback::Failure;
use super::hub::Subscription;
use super::schedule::window;
use super::server::Server;
use super::sharing::{Conn, Rider};
use super::tsprobe::{self, Can, How, Plan, StreamInfo, Tables};
use super::vodwatch::movie_of;
use super::{livemux, now, store};

/// Viewers one device may hold at once: a page, and a second tab or a
/// moment of overlap while changing channel.
pub const PER_DEVICE: usize = 3;
/// Viewers the service holds in all.
pub const MAX_VIEWERS: usize = 16;
/// Channels one device may start within a minute: quick zapping, not a
/// loop.
pub const STARTS_PER_MINUTE: usize = 40;
/// A viewer nobody reads from is let go after this long. Long enough to
/// change audio track (a new reading), short enough that a closed tab
/// frees the connection soon.
pub const IDLE: Duration = Duration::from_secs(15);
/// How long the stream is read for its tables before giving up and
/// passing it on unread.
const PROBE_WAIT: Duration = Duration::from_secs(6);
const PROBE_BYTES: usize = 4 << 20;
/// How long what a channel carries is remembered.
const INFO_FRESH: Duration = Duration::from_secs(6 * 3600);
/// How far ahead a recording that needs the connection is mentioned.
pub const WARN_AHEAD_SECS: i64 = 120;
/// How long an ended viewer is kept, so its page can ask why.
pub(super) const KEEP_ENDED: Duration = Duration::from_secs(60);

// ------------------------------------------------------------- the rules

/// What a viewer that needs a connection of its own does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Connect.
    Go,
    /// End the viewers on the service's connection to this feed, then
    /// connect in its place ("Watch here anyway").
    TakeOver(i64),
    Refuse(Refusal),
}

/// Why a viewer cannot have a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Recordings hold the connections; these feeds could be watched for
    /// nothing. Never overridden.
    Recording {
        feeds: Vec<i64>,
        recordings: Vec<i64>,
    },
    /// Other browsers watch on the service's connections (these feeds);
    /// "anyway" ends one.
    Viewers { feeds: Vec<i64> },
    /// Other devices hold the account's connections; "anyway" connects as
    /// the app's player would.
    InUse { active: i64, max: i64 },
}

fn is_viewer(r: &Rider) -> bool {
    r.stop == i64::MAX
}

/// Whether a viewer may have a connection of its own, by the account's
/// count (`slots`), the service's own connections (`own`) and how many of
/// those closed lately (`released`: a provider may still be counting
/// them), and whether the person said "anyway".
pub fn decide(slots: Slots, own: &[Conn], released: usize, anyway: bool) -> Decision {
    let Slots::Full { active, max } = slots else {
        return Decision::Go;
    };
    // Connections the provider counts beyond the service's own that the
    // service closed a moment ago: zapping from one channel to the next
    // must not find its own old connection in the way.
    // And one of its own that everybody has just left, which is closing.
    let others = (active - own.len() as i64).max(0);
    let leaving = own.iter().filter(|c| c.riders.is_empty()).count() as i64;
    let stale = others.min(released as i64) + leaving;
    if active - stale < max {
        return Decision::Go;
    }
    let recording: Vec<&Conn> = own
        .iter()
        .filter(|c| c.riders.iter().any(|r| !is_viewer(r)))
        .collect();
    let watching: Vec<i64> = own
        .iter()
        .filter(|c| !c.riders.is_empty() && c.riders.iter().all(is_viewer))
        .map(|c| c.feed)
        .collect();
    if let (true, Some(&feed)) = (anyway, watching.first()) {
        return Decision::TakeOver(feed);
    }
    if anyway && recording.is_empty() {
        return Decision::Go;
    }
    Decision::Refuse(
        if !recording.is_empty() && (watching.is_empty() || anyway) {
            Refusal::Recording {
                feeds: recording.iter().map(|c| c.feed).collect(),
                recordings: recording
                    .iter()
                    .flat_map(|c| c.riders.iter().filter(|r| !is_viewer(r)).map(|r| r.id))
                    .collect(),
            }
        } else if !watching.is_empty() {
            Refusal::Viewers { feeds: watching }
        } else {
            Refusal::InUse { active, max }
        },
    )
}

/// A recording as [`notice`] sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Due {
    pub id: i64,
    pub feeds: Vec<i64>,
    /// Its padded start, its programme's start, and the end of its window.
    pub from: i64,
    pub start: i64,
    pub until: i64,
    /// Running, and waiting for a connection.
    pub waiting: bool,
    /// Running and capturing.
    pub capturing: bool,
}

/// The recording the viewer on `mine` is in the way of, if any: due within
/// [`WARN_AHEAD_SECS`] (or running and waiting), on a feed the service has
/// no connection to, on an account with no connection to spare (`max`, the
/// account's limit, against `open`, the service's connections; a recording
/// already waiting has found that out for itself), and on a connection that
/// would be free for it but for the viewers on it.
pub fn notice<'a>(
    now: i64,
    mine: &Conn,
    open_feeds: &[i64],
    due: &'a [Due],
    max: Option<u32>,
    open: usize,
) -> Option<&'a Due> {
    let full = max.is_some_and(|m| open >= m as usize);
    due.iter()
        .filter(|d| d.until > now && d.from <= now + WARN_AHEAD_SECS && !d.capturing)
        .filter(|d| !d.feeds.iter().any(|f| open_feeds.contains(f)))
        .filter(|d| d.waiting || (d.from > now && full))
        .filter(|d| {
            mine.riders
                .iter()
                .filter(|r| !is_viewer(r))
                .all(|r| r.stop <= d.start.max(now))
        })
        .min_by_key(|d| (d.from, d.id))
}

// ------------------------------------------------------------ the viewers

/// Why a viewer stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// The feed failed, after delivering for this many seconds.
    Feed(Failure, i64),
    /// Another browser said "Watch here anyway".
    TakenOver,
}

struct Viewer {
    device: String,
    stream_id: i64,
    rider: i64,
    can: Can,
    info: StreamInfo,
    /// The audio stream played, an index into `info.audio`.
    audio: Option<usize>,
    how: How,
    /// The place on the hub, while nobody reads it.
    sub: Option<Subscription>,
    /// What was read for the tables, sent ahead of the first reading.
    head: Option<Bytes>,
    /// Bumped by each reading; an older one stops when it sees a newer.
    turn: watch::Sender<u64>,
    reading: bool,
    idle_since: Instant,
    ended: Option<(Ended, Instant)>,
}

/// The browsers watching, and what is known of the channels they watched.
#[derive(Default)]
pub struct Viewers {
    map: Mutex<HashMap<String, Viewer>>,
    infos: Mutex<HashMap<i64, (Instant, StreamInfo)>>,
    starts: Mutex<HashMap<String, VecDeque<Instant>>>,
    next_rider: AtomicI64,
    /// Told when a reading gives its place back.
    returned: Notify,
    reaper: OnceLock<()>,
}

impl Viewers {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, Viewer>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Viewers holding a place now.
    pub fn count(&self) -> usize {
        self.map().values().filter(|v| v.ended.is_none()).count()
    }

    /// A rider id no recording has: recordings' are their row ids, from 1.
    pub(super) fn rider_id(&self) -> i64 {
        -1 - self.next_rider.fetch_add(1, Ordering::Relaxed)
    }

    fn known(&self, stream_id: i64) -> Option<StreamInfo> {
        let m = self.infos.lock().unwrap_or_else(|e| e.into_inner());
        m.get(&stream_id)
            .filter(|(at, _)| at.elapsed() < INFO_FRESH)
            .map(|(_, i)| i.clone())
    }

    fn learnt(&self, stream_id: i64, info: &StreamInfo) {
        let mut m = self.infos.lock().unwrap_or_else(|e| e.into_inner());
        m.retain(|_, (at, _)| at.elapsed() < INFO_FRESH);
        m.insert(stream_id, (Instant::now(), info.clone()));
    }

    /// Whether `device` may start another viewer now, with `more_here` and
    /// `more_in_all` movie viewers besides these (`super::vodwatch`): the
    /// limits count both. Counts the start.
    pub(super) fn admit(
        &self,
        device: &str,
        more_here: usize,
        more_in_all: usize,
    ) -> Result<(), &'static str> {
        {
            let map = self.map();
            let live = map.values().filter(|v| v.ended.is_none());
            if live.clone().filter(|v| v.device == device).count() + more_here >= PER_DEVICE {
                return Err(
                    "This device is already watching as many channels as it may. Stop one first.",
                );
            }
            if live.count() + more_in_all >= MAX_VIEWERS {
                return Err("The service is already streaming to as many browsers as it may.");
            }
        }
        let mut starts = self.starts.lock().unwrap_or_else(|e| e.into_inner());
        let q = starts.entry(device.to_string()).or_default();
        while q
            .front()
            .is_some_and(|t| t.elapsed() >= Duration::from_secs(60))
        {
            q.pop_front();
        }
        if q.len() >= STARTS_PER_MINUTE {
            return Err("Too many channels started in a minute. Wait a moment.");
        }
        q.push_back(Instant::now());
        Ok(())
    }

    /// Ends viewer `id` of `device`, if it is that device's.
    /// Its rider, when there was one to stop.
    fn stop(&self, device: &str, id: &str) -> Option<i64> {
        let gone = {
            let mut map = self.map();
            match map.get(id) {
                Some(v) if v.device == device => map.remove(id),
                _ => None,
            }
        };
        gone.map(|v| {
            v.turn.send_replace(u64::MAX);
            v.rider
        })
    }

    /// Ends every viewer on the service's connection to `feed`, saying
    /// another browser took it.
    fn take_over(&self, feed: i64) {
        let mut map = self.map();
        for v in map.values_mut() {
            if v.stream_id == feed && v.ended.is_none() {
                v.ended = Some((Ended::TakenOver, Instant::now()));
                v.sub = None;
                v.turn.send_replace(u64::MAX);
            }
        }
    }

    /// A connection among `conns` that carries only viewers, all of them
    /// `device`'s.
    fn only_mine(&self, conns: &[Conn], device: &str) -> Option<i64> {
        let map = self.map();
        let owner = |rider: i64| {
            map.values()
                .find(|v| v.rider == rider && v.ended.is_none())
                .map(|v| v.device.as_str())
        };
        conns
            .iter()
            .find(|c| {
                !c.riders.is_empty()
                    && c.riders
                        .iter()
                        .all(|r| is_viewer(r) && owner(r.id) == Some(device))
            })
            .map(|c| c.feed)
    }

    /// Lets go of viewers nobody reads, and forgets ended ones.
    fn reap(&self) {
        let mut map = self.map();
        map.retain(|_, v| match v.ended {
            Some((_, at)) => at.elapsed() < KEEP_ENDED,
            None => v.reading || v.idle_since.elapsed() < IDLE,
        });
    }
}

/// Which device a caller is, for the limits and for whose viewer is whose.
pub(super) fn device_of(c: &Caller) -> String {
    match c {
        Caller::App => "app".into(),
        Caller::Device(id) => format!("device:{id}"),
    }
}

pub(super) fn new_id() -> String {
    let mut b = [0u8; 16];
    if getrandom::fill(&mut b).is_err() {
        // No randomness: the id still names the viewer, and only its own
        // device may use it.
        let t = super::now_ms() as u128;
        b = (t ^ (u128::from(std::process::id()) << 64)).to_le_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ------------------------------------------------------------------- DTOs

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct CanDto {
    hevc: bool,
    ac3: bool,
    eac3: bool,
}

impl From<CanDto> for Can {
    fn from(c: CanDto) -> Self {
        Can {
            hevc: c.hevc,
            ac3: c.ac3,
            eac3: c.eac3,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WatchRequest {
    stream_id: i64,
    #[serde(default)]
    anyway: bool,
    /// The viewer this one takes over from (changing channel): it is
    /// stopped first, so its connection is free for this one.
    #[serde(default)]
    replaces: Option<String>,
    #[serde(default)]
    can: CanDto,
    #[serde(default)]
    audio: Option<usize>,
    /// The app's own player, which plays whatever the channel carries: the
    /// stream is passed on as it comes, every track in it, and nothing is
    /// refused for its codecs. The app's alone.
    #[serde(default)]
    player: bool,
    /// Only a connection the service already has (a recording's): never a
    /// new one. The app's player tuning to a channel being recorded.
    #[serde(default)]
    only_shared: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TrackDto {
    index: usize,
    codec: &'static str,
    lang: Option<String>,
    described: bool,
    impaired: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WatchDto {
    viewer: String,
    /// Where to read it: `/api/stream/{viewer}`.
    stream: String,
    video: Option<&'static str>,
    audio: Vec<TrackDto>,
    audio_index: Option<usize>,
    /// The sound is being encoded to AAC on the way.
    converting: bool,
    /// The sound is left out, and why.
    silent: Option<String>,
    /// Joined a connection the service already had.
    shared: bool,
    notice: Option<NoticeDto>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct NoticeDto {
    recording_id: i64,
    title: String,
    channel: String,
    stream_id: i64,
    /// The padded start, and the programme's.
    from: i64,
    start: i64,
    /// It has started, and waits for the connection.
    waiting: bool,
}

/// How a stream stopped, in the terms the app's player uses
/// (`ipc::ending`): the page shows the same words.
#[derive(Debug, Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub(super) struct EndingDto {
    pub(super) kind: &'static str,
    pub(super) status: Option<u16>,
    pub(super) limit: bool,
}

pub(super) fn ending_of(f: Failure) -> EndingDto {
    let (kind, status, limit) = match f {
        Failure::Ended => ("dropped", None, false),
        Failure::Refused { status, limit } => ("refused", Some(status), f.is_limit() || limit),
        Failure::NotVideo => ("notVideo", None, false),
        Failure::Stalled | Failure::Unreachable => ("network", None, false),
    };
    EndingDto {
        kind,
        status,
        limit,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RefusalDto {
    pub(super) error: String,
    /// `recording`, `viewers`, `inUse`, `notPlayable`, `limit`, `feed`,
    /// `channel`, and for a movie `movie`.
    pub(super) reason: &'static str,
    /// "Watch here anyway" may be offered.
    pub(super) anyway: bool,
    pub(super) slots: Option<SlotsDto>,
    pub(super) ending: Option<EndingDto>,
    /// Channels the service has open, watchable at no cost.
    pub(super) watch_instead: Vec<i64>,
}

#[derive(Debug, Serialize, Clone, Copy)]
pub(super) struct SlotsDto {
    active: i64,
    max: i64,
}

pub(super) fn refused(status: StatusCode, r: RefusalDto) -> Response {
    (status, [(header::CACHE_CONTROL, "no-store")], Json(r)).into_response()
}

pub(super) fn plain(
    status: StatusCode,
    reason: &'static str,
    error: impl Into<String>,
) -> Response {
    refused(
        status,
        RefusalDto {
            error: error.into(),
            reason,
            anyway: false,
            slots: None,
            ending: None,
            watch_instead: Vec::new(),
        },
    )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StatusDto {
    /// Why it stopped, once it has.
    pub(super) ended: Option<EndingDto>,
    /// Another browser said "Watch here anyway".
    pub(super) taken_over: bool,
    pub(super) notice: Option<NoticeDto>,
}

// --------------------------------------------------------------- handlers

pub fn routes() -> Router<Arc<Server>> {
    Router::new()
        .route("/api/watch", post(start))
        .route("/api/watch/{id}", get(status).delete(stop))
        .route("/api/stream/{id}", get(stream))
}

/// A channel's name, or a movie's for a movie viewer's feed, for a
/// sentence.
fn channel_name(stream_id: i64) -> String {
    if let Some(movie) = movie_of(stream_id) {
        return crate::catalog::reads::movies_by_ids(&[movie])
            .ok()
            .and_then(|v| v.into_iter().next())
            .map_or_else(|| "a movie".to_string(), |m| format!("“{}”", m.clean_name));
    }
    crate::catalog::reads::channels_by_ids(&[stream_id])
        .ok()
        .and_then(|v| v.into_iter().next())
        .map_or_else(|| format!("channel {stream_id}"), |c| c.clean_name)
}

fn names(feeds: &[i64]) -> String {
    let n: Vec<String> = feeds.iter().map(|&f| channel_name(f)).collect();
    n.join(", ")
}

/// The feeds among `feeds` a viewer could join: channels, not movies.
fn channels(feeds: &[i64]) -> Vec<i64> {
    feeds
        .iter()
        .copied()
        .filter(|&f| movie_of(f).is_none())
        .collect()
}

fn refusal_response(s: &Server, r: Refusal) -> Response {
    match r {
        Refusal::Recording { feeds, recordings } => {
            let titles: Vec<String> = recordings
                .iter()
                .filter_map(|&id| s.get(id).ok())
                .map(|r| format!("“{}” on {}", r.title, r.channel_name))
                .collect();
            let what = if titles.is_empty() {
                "recordings".to_string()
            } else {
                format!("recording {}", titles.join(" and "))
            };
            let error = format!(
                "Every connection the account allows is in use, {what}. Watching {} costs no extra connection.",
                if feeds.len() == 1 { "that channel" } else { "one of those channels" }
            );
            refused(
                StatusCode::CONFLICT,
                RefusalDto {
                    error,
                    reason: "recording",
                    anyway: false,
                    slots: None,
                    ending: None,
                    watch_instead: channels(&feeds),
                },
            )
        }
        Refusal::Viewers { feeds } => refused(
            StatusCode::CONFLICT,
            RefusalDto {
                error: format!(
                    "Another browser is watching {} on the account's connection.",
                    names(&feeds)
                ),
                reason: "viewers",
                anyway: true,
                slots: None,
                ending: None,
                watch_instead: channels(&feeds),
            },
        ),
        Refusal::InUse { active, max } => refused(
            StatusCode::CONFLICT,
            RefusalDto {
                error: format!(
                    "Your account is in use on another device ({active} of {max} connection{}).",
                    if max == 1 { "" } else { "s" }
                ),
                reason: "inUse",
                anyway: true,
                slots: Some(SlotsDto { active, max }),
                ending: None,
                watch_instead: Vec::new(),
            },
        ),
    }
}

/// Whether a viewer may have a connection of its own: the budget asked with
/// every connection the service holds, channels' and movies' alike
/// ([`Server::own_conns`]). On "anyway" (or for this device's own other
/// viewer) the viewers in the way are ended first, and their connection
/// given a moment to close. `Err`: the answer to send instead.
pub(super) async fn may_connect(
    s: &Arc<Server>,
    anyway: bool,
    device: &str,
) -> Result<(), Box<Response>> {
    let conns = s.own_conns();
    let slots = s.budget.check(s.open_streams()).await;
    let released = s.hubs.released_recently() + s.vod.released_recently();
    let mut decision = decide(slots, &conns, released, anyway);
    // This browser's own other viewer (a page reloaded, a second tab) is
    // not another device to ask about: the newer one has it, as the app's
    // one player would.
    if let Decision::Refuse(Refusal::Viewers { .. }) = decision {
        if let Some(feed) = s
            .viewers
            .only_mine(&conns, device)
            .or_else(|| s.vod.only_mine(&conns, device))
        {
            decision = Decision::TakeOver(feed);
        }
    }
    match decision {
        Decision::Go => Ok(()),
        Decision::TakeOver(feed) => {
            // Two browsers may each watch the same movie, each on a
            // connection of its own: one is ended, so wait for one fewer.
            let on_it = |s: &Server| s.own_conns().iter().filter(|c| c.feed == feed).count();
            let before = on_it(s);
            if movie_of(feed).is_some() {
                s.vod.end(feed);
            } else {
                s.viewers.take_over(feed);
            }
            // The connection closes once its viewers have let go; the
            // provider is then given a moment, as for a recording.
            let t = Instant::now();
            while (s.hubs.closing() || on_it(s) >= before.max(1))
                && t.elapsed() < Duration::from_secs(5)
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Ok(())
        }
        Decision::Refuse(r) => Err(Box::new(refusal_response(s, r))),
    }
}

/// Stops `device`'s viewer `old`, a channel's or a movie's, and waits a
/// moment for the connection it leaves to close (when nobody else is on
/// it), so that the viewer replacing it may have it.
pub(super) async fn let_go(s: &Arc<Server>, device: &str, old: &str) {
    let Some(rider) = s
        .viewers
        .stop(device, old)
        .or_else(|| s.vod.stop(device, old))
    else {
        return;
    };
    let t = Instant::now();
    while (s.hubs.closing()
        || s.own_conns()
            .iter()
            .any(|c| c.riders.is_empty() || c.riders.iter().any(|r| r.id == rider)))
        && t.elapsed() < Duration::from_secs(3)
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A connection for a viewer of `stream_id`: a place on one the service has,
/// or a new one the budget allows. `Err`: the answer to send instead.
async fn place(
    s: &Arc<Server>,
    stream_id: i64,
    me: Rider,
    anyway: bool,
    device: &str,
) -> Result<(Subscription, bool), Box<Response>> {
    if let Some((_, sub)) = s.hubs.join(&[stream_id], me) {
        return Ok((sub, true));
    }
    may_connect(s, anyway, device).await?;
    let playable = match s.playable(stream_id) {
        Ok(p) => p,
        Err(e) => {
            return Err(Box::new(plain(
                StatusCode::NOT_FOUND,
                "channel",
                format!(
                    "Can't play this channel: {}",
                    crate::catalog::sync_engine::scrub(&format!("{e:#}"))
                ),
            )))
        }
    };
    Ok((s.hubs.open(stream_id, playable, me), false))
}

/// Reads the stream's first packets until its tables have gone past. The
/// bytes read, and what they said (`None`: nothing readable in time, so the
/// browser's player is left to make what it can of it).
async fn probe(sub: &mut Subscription) -> Result<(Bytes, Option<StreamInfo>), (Failure, i64)> {
    let mut t = Tables::new();
    let mut head = BytesMut::new();
    let deadline = tokio::time::Instant::now() + PROBE_WAIT;
    loop {
        let chunk = tokio::select! {
            c = sub.recv() => c,
            _ = tokio::time::sleep_until(deadline) => return Ok((head.freeze(), None)),
        };
        let Some(c) = chunk else {
            return Err(sub.ended().unwrap_or((Failure::Ended, 0)));
        };
        head.extend_from_slice(&c);
        if let Some(info) = t.feed(&c) {
            let info = info.clone();
            return Ok((head.freeze(), Some(info)));
        }
        if head.len() > PROBE_BYTES {
            return Ok((head.freeze(), None));
        }
    }
}

fn tracks(info: &StreamInfo) -> Vec<TrackDto> {
    info.audio
        .iter()
        .enumerate()
        .map(|(index, a)| TrackDto {
            index,
            codec: a.codec.name(),
            lang: a.lang.clone(),
            described: a.described,
            impaired: a.impaired,
        })
        .collect()
}

async fn start(
    State(s): State<Arc<Server>>,
    Extension(caller): Extension<Caller>,
    body: Result<Json<WatchRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(req)) = body else {
        return plain(
            StatusCode::BAD_REQUEST,
            "channel",
            "Not a request to watch a channel.",
        );
    };
    let device = device_of(&caller);
    if req.player && caller != Caller::App {
        return plain(
            StatusCode::FORBIDDEN,
            "channel",
            "Only the app on this computer plays the stream as it comes.",
        );
    }
    if let Some(old) = req.replaces.as_deref() {
        // Its reading lets go of its place in a moment; the connection it
        // leaves (when nobody else is on it) is then this one's to take.
        let_go(&s, &device, old).await;
    }
    if let Err(why) = s
        .viewers
        .admit(&device, s.vod.count_for(&device), s.vod.len())
    {
        return plain(StatusCode::TOO_MANY_REQUESTS, "limit", why);
    }
    start_reaper(&s);
    let can = Can::from(req.can);
    let encoder = livemux::can_encode_aac();
    if let Some(info) = s.viewers.known(req.stream_id).filter(|_| !req.player) {
        if let Plan::Refuse(why) = tsprobe::plan(&info, can, None, encoder) {
            return plain(StatusCode::UNPROCESSABLE_ENTITY, "notPlayable", why);
        }
    }

    let rider = s.viewers.rider_id();
    let me = Rider::viewer(rider);
    let placed = if req.only_shared {
        s.hubs
            .join(&[req.stream_id], me)
            .map(|(_, sub)| (sub, true))
            .ok_or_else(|| {
                Box::new(plain(
                    StatusCode::CONFLICT,
                    "notOpen",
                    "The service has no connection to this channel.",
                ))
            })
    } else {
        place(&s, req.stream_id, me, req.anyway, &device).await
    };
    let (mut sub, shared) = match placed {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let (head, info) = match probe(&mut sub).await {
        Ok(h) => h,
        Err((f, _)) => {
            return refused(
                StatusCode::BAD_GATEWAY,
                RefusalDto {
                    error: format!("The channel didn't start: {}.", f.describe()),
                    reason: "feed",
                    anyway: f.is_limit() || matches!(f, Failure::Ended),
                    slots: None,
                    ending: Some(ending_of(f)),
                    watch_instead: Vec::new(),
                },
            )
        }
    };
    let info = match info {
        Some(i) => {
            s.viewers.learnt(req.stream_id, &i);
            i
        }
        None => StreamInfo::default(),
    };
    // Nothing readable in the tables, or the app's player, which plays
    // anything: passed on as it comes.
    let (how, audio, silent) = if req.player || info == StreamInfo::default() {
        (How::Pass, None, None)
    } else {
        match tsprobe::plan(&info, can, req.audio, encoder) {
            Plan::Refuse(why) => {
                return plain(StatusCode::UNPROCESSABLE_ENTITY, "notPlayable", why);
            }
            Plan::Play { how, audio, silent } => (how, audio, silent),
        }
    };

    let id = new_id();
    let notice = notice_for(&s, req.stream_id, rider).await;
    let dto = WatchDto {
        viewer: id.clone(),
        stream: format!("/api/stream/{id}"),
        video: info.video.map(|(_, v)| v.name()),
        audio: tracks(&info),
        audio_index: audio,
        converting: matches!(how, How::Remux { encode: true, .. }),
        silent,
        shared,
        notice,
    };
    s.viewers.map().insert(
        id,
        Viewer {
            device,
            stream_id: req.stream_id,
            rider,
            can,
            info,
            audio,
            how,
            sub: Some(sub),
            head: Some(head),
            turn: watch::channel(0).0,
            reading: false,
            idle_since: Instant::now(),
            ended: None,
        },
    );
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(dto),
    )
        .into_response()
}

fn start_reaper(s: &Arc<Server>) {
    let weak: Weak<Server> = Arc::downgrade(s);
    s.viewers.reaper.get_or_init(move || {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let Some(s) = weak.upgrade() else { return };
                s.viewers.reap();
            }
        });
    });
}

/// What the page is told about the recording its viewer (on a channel's
/// feed or a movie's) is in the way of.
pub(super) async fn notice_for(s: &Arc<Server>, feed: i64, rider: i64) -> Option<NoticeDto> {
    let conns = s.own_conns();
    let mine = conns
        .iter()
        .find(|c| c.feed == feed && c.riders.iter().any(|r| r.id == rider))?;
    let t = now();
    let recs = s.db(store::list).ok()?;
    let live: HashMap<i64, bool> =
        s.live(|m| m.iter().map(|(&id, l)| (id, l.waiting.is_some())).collect());
    let due: Vec<Due> = recs
        .iter()
        .filter(|r| {
            matches!(
                r.state,
                RecordingState::Scheduled | RecordingState::Recording
            )
        })
        .map(|r| {
            let (from, until) = window(r.start, r.stop, r.pad_before, r.pad_after);
            let running = live.get(&r.id).copied();
            Due {
                id: r.id,
                feeds: std::iter::once(r.stream_id)
                    .chain(r.backup_stream_id)
                    .collect(),
                from,
                start: r.start,
                until,
                waiting: running == Some(true),
                capturing: running == Some(false),
            }
        })
        .collect();
    let open: Vec<i64> = conns.iter().map(|c| c.feed).collect();
    let max = s.account_max().await;
    let d = notice(t, mine, &open, &due, max, s.open_streams())?;
    let r = recs.iter().find(|r| r.id == d.id)?;
    Some(NoticeDto {
        recording_id: r.id,
        title: r.title.clone(),
        channel: r.channel_name.clone(),
        stream_id: r.stream_id,
        from: d.from,
        start: d.start,
        waiting: d.waiting,
    })
}

async fn status(
    State(s): State<Arc<Server>>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> Response {
    let device = device_of(&caller);
    let found = s
        .viewers
        .map()
        .get(&id)
        .filter(|v| v.device == device)
        .map(|v| (v.stream_id, v.rider, v.ended));
    let Some((feed, rider, ended)) = found else {
        return plain(StatusCode::NOT_FOUND, "channel", "No such viewer.");
    };
    let dto = match ended {
        Some((Ended::Feed(f, _), _)) => StatusDto {
            ended: Some(ending_of(f)),
            taken_over: false,
            notice: None,
        },
        Some((Ended::TakenOver, _)) => StatusDto {
            ended: Some(EndingDto {
                kind: "dropped",
                status: None,
                limit: false,
            }),
            taken_over: true,
            notice: None,
        },
        None => StatusDto {
            ended: None,
            taken_over: false,
            notice: notice_for(&s, feed, rider).await,
        },
    };
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(dto),
    )
        .into_response()
}

async fn stop(
    State(s): State<Arc<Server>>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> Response {
    s.viewers.stop(&device_of(&caller), &id);
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Debug, Deserialize)]
struct StreamQuery {
    audio: Option<usize>,
}

/// The chunks of a reading, as a response body (a recording's live stream,
/// `strm`, is sent the same way).
pub(super) struct Chunks(pub(super) mpsc::Receiver<Bytes>);

impl futures_core::Stream for Chunks {
    type Item = Result<Bytes, std::io::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.poll_recv(cx).map(|o| o.map(Ok))
    }
}

/// A stream is only ever read by this origin's own page: a page elsewhere
/// has no business making a browser fetch it, cookie or not.
pub(super) fn from_another_site(h: &axum::http::HeaderMap) -> bool {
    let site = h.get("sec-fetch-site").and_then(|v| v.to_str().ok());
    site.is_some_and(|v| v != "same-origin" && v != "none")
}

/// How a reading ended.
enum Finish {
    /// The browser left, or a newer reading took over: the place is kept
    /// for a while.
    Left(Subscription),
    /// The feed failed.
    Failed(Failure, i64),
    /// The viewer was stopped.
    Stopped,
}

async fn stream(
    State(s): State<Arc<Server>>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Query(q): Query<StreamQuery>,
    req: Request,
) -> Response {
    if from_another_site(req.headers()) {
        return plain(
            StatusCode::FORBIDDEN,
            "channel",
            "refused: the request came from another site",
        );
    }
    let device = device_of(&caller);
    let encoder = livemux::can_encode_aac();

    // Take the place: from the map, or from an older reading, which is told
    // to stop and gives it back.
    let deadline = Instant::now() + Duration::from_secs(3);
    let (sub, head, how, turn, my_turn) = loop {
        let waiter = s.viewers.returned.notified();
        {
            let mut map = s.viewers.map();
            let Some(v) = map.get_mut(&id).filter(|v| v.device == device) else {
                return plain(StatusCode::NOT_FOUND, "channel", "No such viewer.");
            };
            if v.ended.is_some() {
                return plain(StatusCode::GONE, "feed", "This stream has ended.");
            }
            if let Some(sub) = v.sub.take() {
                if let Some(want) = q.audio.filter(|&a| Some(a) != v.audio) {
                    if let Plan::Play { how, audio, .. } =
                        tsprobe::plan(&v.info, v.can, Some(want), encoder)
                    {
                        v.how = how;
                        v.audio = audio;
                    }
                }
                let next = v.turn.borrow().wrapping_add(1);
                v.turn.send_replace(next);
                v.reading = true;
                break (sub, v.head.take(), v.how, v.turn.subscribe(), next);
            }
            // Read elsewhere: that reading stops and hands it back.
            let next = v.turn.borrow().wrapping_add(1);
            v.turn.send_replace(next);
        }
        if Instant::now() >= deadline {
            return plain(
                StatusCode::CONFLICT,
                "channel",
                "This stream is being read elsewhere.",
            );
        }
        let _ = tokio::time::timeout(Duration::from_millis(500), waiter).await;
    };

    let (tx, rx) = mpsc::channel::<Bytes>(64);
    let server = Arc::clone(&s);
    tokio::spawn(async move {
        let end = match how {
            How::Pass => pass(sub, head, &tx, turn, my_turn).await,
            How::Remux { audio, encode } => {
                remux(sub, head, &tx, turn, my_turn, audio, encode).await
            }
        };
        let mut map = server.viewers.map();
        if let Some(v) = map.get_mut(&id) {
            if *v.turn.borrow() == my_turn {
                v.reading = false;
                v.idle_since = Instant::now();
            }
            match end {
                Finish::Left(sub) if v.ended.is_none() => v.sub = Some(sub),
                Finish::Failed(f, secs) if v.ended.is_none() => {
                    v.ended = Some((Ended::Feed(f, secs), Instant::now()))
                }
                _ => {}
            }
        }
        drop(map);
        server.viewers.returned.notify_waiters();
    });
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "video/mp2t"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(Chunks(rx)),
    )
        .into_response()
}

/// Whether a newer reading (or a stop) has come.
fn superseded(turn: &watch::Receiver<u64>, mine: u64) -> bool {
    *turn.borrow() != mine
}

/// The stream as it comes.
async fn pass(
    mut sub: Subscription,
    head: Option<Bytes>,
    tx: &mpsc::Sender<Bytes>,
    mut turn: watch::Receiver<u64>,
    mine: u64,
) -> Finish {
    if let Some(h) = head.filter(|h| !h.is_empty()) {
        if tx.send(h).await.is_err() {
            return Finish::Left(sub);
        }
    }
    loop {
        if superseded(&turn, mine) {
            return finish_left(sub, &turn);
        }
        tokio::select! {
            c = sub.recv() => match c {
                // A browser that stopped reading must not keep a newer
                // reading (or a stop) waiting.
                Some(b) => tokio::select! {
                    r = tx.send(b) => if r.is_err() {
                        return Finish::Left(sub);
                    },
                    _ = turn.changed() => {}
                },
                None => {
                    let (f, secs) = sub.ended().unwrap_or((Failure::Ended, 0));
                    return Finish::Failed(f, secs);
                }
            },
            _ = turn.changed() => {}
            _ = tx.closed() => return Finish::Left(sub),
        }
    }
}

fn finish_left(sub: Subscription, turn: &watch::Receiver<u64>) -> Finish {
    if *turn.borrow() == u64::MAX {
        Finish::Stopped
    } else {
        Finish::Left(sub)
    }
}

/// The stream through the remuxer.
async fn remux(
    mut sub: Subscription,
    head: Option<Bytes>,
    tx: &mpsc::Sender<Bytes>,
    mut turn: watch::Receiver<u64>,
    mine: u64,
    audio: Option<u16>,
    encode: bool,
) -> Finish {
    let Ok(livemux::Remuxer { input, mut output }) = livemux::start(audio, encode) else {
        return Finish::Left(sub);
    };
    // What comes out goes to the browser on its own task, so a slow browser
    // holds the remuxer up rather than the feed's reading.
    let out = tx.clone();
    let forward = tokio::spawn(async move {
        while let Some(b) = output.recv().await {
            if out.send(b).await.is_err() {
                return;
            }
        }
    });
    if let Some(h) = head.filter(|h| !h.is_empty()) {
        if input.send(h).await.is_err() {
            forward.abort();
            return Finish::Left(sub);
        }
    }
    let end = loop {
        if superseded(&turn, mine) {
            break finish_left(sub, &turn);
        }
        tokio::select! {
            c = sub.recv() => match c {
                Some(b) => tokio::select! {
                    r = input.send(b) => if r.is_err() {
                        break Finish::Left(sub);
                    },
                    _ = turn.changed() => {}
                },
                None => {
                    let (f, secs) = sub.ended().unwrap_or((Failure::Ended, 0));
                    break Finish::Failed(f, secs);
                }
            },
            _ = turn.changed() => {}
            _ = tx.closed() => break Finish::Left(sub),
        }
    };
    // Ending the input lets the remuxer finish what it holds; a browser
    // that left has dropped its end already.
    drop(input);
    if matches!(end, Finish::Failed(..)) {
        let _ = tokio::time::timeout(Duration::from_secs(5), forward).await;
    } else {
        forward.abort();
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: i64, stop: i64, until: i64) -> Rider {
        Rider { id, stop, until }
    }

    fn conn(feed: i64, riders: Vec<Rider>) -> Conn {
        Conn { feed, riders }
    }

    const FULL: Slots = Slots::Full { active: 1, max: 1 };

    #[test]
    fn a_free_account_or_one_nobody_can_count_lets_a_viewer_connect() {
        assert_eq!(decide(Slots::Free, &[], 0, false), Decision::Go);
        assert_eq!(decide(Slots::Unknown, &[], 0, false), Decision::Go);
    }

    #[test]
    fn a_recordings_connection_is_never_taken_even_anyway() {
        let own = [conn(7, vec![rec(1, 100, 400)])];
        for anyway in [false, true] {
            assert_eq!(
                decide(FULL, &own, 0, anyway),
                Decision::Refuse(Refusal::Recording {
                    feeds: vec![7],
                    recordings: vec![1]
                }),
                "anyway: {anyway}"
            );
        }
        // Nor one a viewer shares with a recording.
        let shared = [conn(7, vec![rec(1, 100, 400), Rider::viewer(-1)])];
        assert!(matches!(
            decide(FULL, &shared, 0, true),
            Decision::Refuse(Refusal::Recording { .. })
        ));
    }

    #[test]
    fn another_browser_is_named_and_anyway_takes_its_place() {
        let own = [conn(7, vec![Rider::viewer(-1)])];
        assert_eq!(
            decide(FULL, &own, 0, false),
            Decision::Refuse(Refusal::Viewers { feeds: vec![7] })
        );
        assert_eq!(decide(FULL, &own, 0, true), Decision::TakeOver(7));
    }

    #[test]
    fn another_device_is_said_and_anyway_connects_when_no_recording_is_at_stake() {
        assert_eq!(
            decide(FULL, &[], 0, false),
            Decision::Refuse(Refusal::InUse { active: 1, max: 1 })
        );
        assert_eq!(decide(FULL, &[], 0, true), Decision::Go);
        // Another device and a recording: connecting anyway might end the
        // recording's connection rather than the other device's.
        let two = Slots::Full { active: 2, max: 2 };
        let own = [conn(7, vec![rec(1, 100, 400)])];
        assert!(matches!(
            decide(two, &own, 0, true),
            Decision::Refuse(Refusal::Recording { .. })
        ));
    }

    #[test]
    fn a_connection_the_service_just_closed_does_not_count_against_the_next_channel() {
        // Zapping: the old channel's connection closed a moment ago and the
        // provider still counts it.
        assert_eq!(decide(FULL, &[], 1, false), Decision::Go);
        // Nor one everybody has just left, which is closing.
        assert_eq!(decide(FULL, &[conn(7, vec![])], 0, false), Decision::Go);
        // But a connection closed lately is no excuse when the service still
        // holds as many as the account allows.
        let own = [conn(7, vec![Rider::viewer(-1)])];
        assert!(matches!(
            decide(FULL, &own, 1, false),
            Decision::Refuse(Refusal::Viewers { .. })
        ));
    }

    fn due(id: i64, feed: i64, from: i64, start: i64) -> Due {
        Due {
            id,
            feeds: vec![feed],
            from,
            start,
            until: start + 3600,
            waiting: false,
            capturing: false,
        }
    }

    #[test]
    fn a_viewer_is_told_of_a_recording_it_would_hold_up() {
        let mine = conn(7, vec![Rider::viewer(-1)]);
        let d = [due(1, 9, 1000, 1060)];
        // Too far ahead yet.
        assert_eq!(notice(800, &mine, &[7], &d, Some(1), 1), None);
        // Two minutes before: said.
        assert_eq!(notice(880, &mine, &[7], &d, Some(1), 1), Some(&d[0]));
        // An account with room for both: nothing to say.
        assert_eq!(notice(880, &mine, &[7], &d, Some(2), 1), None);
        // Same channel: the recording joins the viewer's connection.
        let same = [due(1, 7, 1000, 1060)];
        assert_eq!(notice(880, &mine, &[7], &same, Some(1), 1), None);
        // Already waiting: said, whatever the count.
        let mut waiting = d.clone();
        waiting[0].waiting = true;
        assert_eq!(
            notice(1010, &mine, &[7], &waiting, None, 1),
            Some(&waiting[0])
        );
        // Recording: nothing to say.
        let mut capturing = d.clone();
        capturing[0].capturing = true;
        assert_eq!(notice(1010, &mine, &[7], &capturing, Some(1), 1), None);
    }

    #[test]
    fn a_viewer_beside_a_programme_still_on_is_not_what_holds_it_up() {
        // The connection carries a recording whose programme runs past the
        // other one's start: without the viewer it would still wait.
        let mine = conn(7, vec![rec(2, 2000, 2300), Rider::viewer(-1)]);
        let d = [due(1, 9, 1000, 1060)];
        assert_eq!(notice(900, &mine, &[7], &d, Some(1), 1), None);
        // One in its padding after would have given way: the viewer is.
        let padding = conn(7, vec![rec(2, 1000, 1300), Rider::viewer(-1)]);
        assert_eq!(notice(900, &padding, &[7], &d, Some(1), 1), Some(&d[0]));
    }
}
