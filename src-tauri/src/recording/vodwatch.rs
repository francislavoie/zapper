//! Watching a movie in a browser signed in to the service
//! (docs/recording.md, "Watching in the browser").
//!
//! As with a channel ([`super::watch`]), the browser never sees a provider
//! address: it asks to watch a movie by its id (`POST /api/vod/watch`), is
//! given a viewer of its own, and reads that viewer's stream from here
//! (`GET /api/vod/stream/{viewer}`).
//!
//! **Connections.** A movie viewer always has a connection of its own:
//! nobody is ever at the same place in a film as somebody else, so there is
//! nothing to share. It is asked of the same budget as a channel's
//! ([`watch::decide`]), as one of the service's own connections
//! ([`Server::own_conns`]), on a feed of its own ([`movie_feed`], which
//! never names a channel) with a rider whose programme never ends, so the
//! sharing rules never close it. What is the same as live:
//!
//! - on a full account it is refused with the reason, and "Watch here
//!   anyway" takes the place of another browser's viewer (a channel's or a
//!   movie's), or, when the service holds nothing, connects as the app's
//!   player would. A recording's connection is never taken;
//! - a recording due on another channel waits for it, and its page is told
//!   ([`watch::notice`]);
//! - a viewer that leaves gives the connection up.
//!
//! The account sees one connection per viewer: the file is read one
//! response at a time ([`HttpUpstream`]), and a new request on a viewer (the
//! browser's player seeking) ends the one before, its response dropped
//! before the next is asked for.
//!
//! **What the browser can play** is read from the movie's details, before
//! any connection ([`vod_mode`]); a movie's stream is not probed as a
//! channel's is. An MP4 with H.264 video and AAC or MP3 sound, and no
//! subtitles, is played as it is ("direct"): the browser's own player asks
//! for byte ranges, which are passed on to the provider. Anything else, or
//! a file whose details are not known, goes through the remuxer
//! ([`super::vodmux`], "remux"): the watch opens the file once to learn what
//! is in it (its length, sounds and subtitles, which the answer lists, and
//! whether a browser plays its video: [`remux_plan`]), and each stream
//! request (`?from=<secs>&audio=<index>`) starts a remux from that time, as
//! MPEG-TS, its sound converted to AAC when the browser lacks it. The
//! answer comes once the remux has found the keyframe it starts on, which
//! it names (`X-Start`, seconds into the film). Seeking past what the
//! browser holds is a new request, which ends the remux before it (and the
//! file's response with it) before the next starts.
//!
//! **Subtitles** come from the same read: the remux hands over the file's
//! text subtitles as it reads them, kept per viewer for its current remux
//! ([`Cues`]), and the page fetches them as WebVTT
//! (`GET /api/vod/subtitles/{viewer}/{index}?after=<secs>`, in the film's
//! time). Pictures for subtitles (DVD, Blu-ray, DVB) are listed, as such,
//! and not shown.
//!
//! **Limits** are live's, counted together: a device's
//! [`watch::PER_DEVICE`] viewers and [`watch::STARTS_PER_MINUTE`] starts,
//! and the service's [`watch::MAX_VIEWERS`], channels and movies alike. A
//! viewer nobody has read from for [`IDLE`] is let go, unless its page has
//! asked after it (`GET /api/vod/watch/{viewer}`) within [`ASKED_IDLE`]: a
//! browser that holds all it needs for now reads nothing, but its page still
//! asks.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use axum::body::Body;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::sync::watch as tokio_watch;
use tokio::sync::{mpsc, oneshot, Notify};

use super::access::Caller;
use super::fallback::Failure;
use super::hub::SETTLE_SECS;
use super::livemux::can_encode_aac;
use super::server::{Resolve, Server};
use super::sharing::{Conn, Rider};
use super::tsprobe::{Can, Video};
use super::upstream::HttpUpstream;
use super::vodmux::{self, Cue, MediaInfo, SubtitleStream, Upstream, VodPlan, Vodmux};
use super::watch::{
    self, device_of, ending_of, new_id, plain, refused, CanDto, Chunks, EndingDto, NoticeDto,
    RefusalDto, StatusDto, IDLE, KEEP_ENDED,
};
use crate::catalog::sync_engine::scrub;
use crate::catalog::vod_details::{self, MovieInfo};
use crate::catalog::{db, reads, vod_store};
use crate::source::{Playable, Source};

// ------------------------------------------------------------- the rules

/// The feed a viewer of movie `movie_id` is on, as the sharing rules see
/// it: channel ids are positive, so a movie's connection never names a
/// channel.
pub fn movie_feed(movie_id: i64) -> i64 {
    -movie_id
}

/// The movie a feed is, when it is one.
pub fn movie_of(feed: i64) -> Option<i64> {
    feed.checked_neg().filter(|&m| m > 0)
}

/// How a movie reaches the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VodMode {
    /// The file as it is, by byte ranges.
    Direct,
    /// Through the remuxer, as MPEG-TS.
    Remux,
}

/// Whether the browser plays movie file of `container` as it is, by what
/// its details say (`info`; `None` when nobody knows): an MP4 (`mp4`, `m4v`,
/// `mov`) with H.264 video, every sound AAC or MP3 (or none listed), and no
/// subtitles listed (they are shown from the remuxer's read). `can`, what
/// the browser says it decodes, does not change the answer: the codecs
/// played direct are the ones every browser plays.
pub fn vod_mode(container: Option<&str>, info: Option<&MovieInfo>, _can: &Can) -> VodMode {
    let is = |s: &str, any: &[&str]| any.iter().any(|a| s.eq_ignore_ascii_case(a));
    let mp4 = container.is_some_and(|c| is(c, &["mp4", "m4v", "mov"]));
    let plays = info.is_some_and(|i| {
        i.video_codec.as_deref().is_some_and(|v| is(v, &["h264"]))
            && i.audio_codecs.iter().all(|a| is(a, &["aac", "mp3"]))
            && i.subtitle_languages.is_empty()
    });
    if mp4 && plays {
        VodMode::Direct
    } else {
        VodMode::Remux
    }
}

/// Whether the browser plays sound `codec` as it is, in the remuxer's
/// MPEG-TS: what the remuxer does not encode to AAC.
fn plays_sound(codec: &str, can: Can) -> bool {
    match codec.to_ascii_lowercase().as_str() {
        "aac" | "mp3" => true,
        "ac3" => can.ac3,
        "eac3" => can.eac3,
        _ => false,
    }
}

/// What the remuxer does with a file (`info`) for a browser that `can`:
/// the video copied, when a browser here plays it (H.264, or HEVC where the
/// browser says so); refused, in live's words, when not. Sound `want` (its
/// number in the file; the first when `None` or not there), encoded to AAC
/// unless the browser plays it; none for a film without.
pub fn remux_plan(info: &MediaInfo, want: Option<usize>, can: &Can) -> Result<VodPlan, String> {
    let video = info.video_codec.as_deref().map(|v| match v {
        "h264" => Video::H264,
        "hevc" => Video::Hevc,
        "mpeg2video" | "mpeg1video" => Video::Mpeg2,
        _ => Video::Other,
    });
    match video {
        Some(Video::H264) => {}
        Some(Video::Hevc) if can.hevc => {}
        Some(v) => {
            return Err(format!(
                "Its video is {}, which {} can’t play. Watch it in the zapper app.",
                v.name(),
                if v == Video::Hevc {
                    "this browser"
                } else {
                    "browsers"
                }
            ))
        }
        None if info.audio.is_empty() => {
            return Err("Its file has neither video nor sound a browser can play.".into())
        }
        None => {}
    }
    let sound = want
        .and_then(|w| info.audio.iter().find(|a| a.index == w))
        .or(info.audio.first());
    Ok(match sound {
        Some(a) => VodPlan {
            audio: Some(a.index),
            encode: !plays_sound(&a.codec, *can),
        },
        None => VodPlan {
            audio: None,
            encode: false,
        },
    })
}

/// [`remux_plan`] for the `audio`th sound the file lists (as the watch
/// answer numbers them). A build without an AAC encoder leaves out sound
/// that needs one.
fn plan_for(info: &MediaInfo, audio: Option<usize>, can: &Can) -> Result<VodPlan, String> {
    let want = audio.and_then(|a| info.audio.get(a)).map(|a| a.index);
    let plan = remux_plan(info, want, can)?;
    Ok(if plan.encode && !can_encode_aac() {
        VodPlan {
            audio: None,
            encode: false,
        }
    } else {
        plan
    })
}

/// A viewer nobody reads from, but whose page has asked after it
/// (`GET /api/vod/watch/{viewer}`) within this long, is kept: the browser
/// holds all it needs for now, and a page in a background tab may ask only
/// about once a minute. Others go after [`IDLE`].
const ASKED_IDLE: Duration = Duration::from_secs(90);

// -------------------------------------------------------------- subtitles

/// A track's cues kept at most.
const MAX_CUES: usize = 2000;
/// A viewer's cues kept at most, all tracks together: a file with many
/// tracks keeps less of each.
const MAX_CUES_TOTAL: usize = 8000;
/// Cues that ended this long before the newest began are let go: a page's
/// player is never that far behind what its remux has read.
const CUES_BEHIND: f64 = 600.0;

/// The text subtitles a viewer's current remux has read, by track (its
/// number in the file), in the film's time.
#[derive(Debug, Default)]
struct Cues {
    /// The reading they come from; a newer one starts afresh.
    turn: u64,
    tracks: HashMap<usize, VecDeque<Cue>>,
}

impl Cues {
    fn new(turn: u64) -> Self {
        Self {
            turn,
            tracks: HashMap::new(),
        }
    }

    /// A track's cues come in order.
    fn push(&mut self, cue: Cue) {
        let newest = cue.start;
        let q = self.tracks.entry(cue.track).or_default();
        q.push_back(cue);
        while q.len() > MAX_CUES || q.front().is_some_and(|c| c.end < newest - CUES_BEHIND) {
            q.pop_front();
        }
        // Over the viewer's share: the earliest cue of any track goes first.
        while self.tracks.values().map(VecDeque::len).sum::<usize>() > MAX_CUES_TOTAL {
            let Some(q) = self
                .tracks
                .values_mut()
                .filter(|q| !q.is_empty())
                .min_by(|a, b| a[0].start.total_cmp(&b[0].start))
            else {
                break;
            };
            q.pop_front();
        }
    }

    /// Track `track`'s cues that end after `after`, as a WebVTT file.
    fn webvtt(&self, track: usize, after: f64) -> String {
        let mut doc = String::from("WEBVTT\n\n");
        for c in self.tracks.get(&track).into_iter().flatten() {
            if c.end > after {
                doc.push_str(&c.to_webvtt());
            }
        }
        doc
    }
}

// ------------------------------------------------------------ the viewers

/// Why a viewer stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// The provider would not give the file.
    Feed(Failure),
    /// Another browser said "Watch here anyway".
    TakenOver,
    /// Its page let it go; gone once its reading has.
    Stopped,
}

struct MovieViewer {
    device: String,
    movie_id: i64,
    rider: i64,
    mode: VodMode,
    /// The file, while nobody reads it (`None` while a reading has it).
    up: Option<HttpUpstream>,
    /// Where the watch asked to start (seconds into the film), the sound it
    /// chose (an index into the answer's list) and what the browser
    /// decodes: a remux's, unless its request says otherwise.
    from: f64,
    audio: Option<usize>,
    can: Can,
    /// The file's subtitle tracks, as the watch answer lists them, and how
    /// many sounds it has.
    subtitles: Vec<SubtitleStream>,
    sounds: usize,
    cues: Cues,
    /// Bumped by each reading; an older one stops when it sees a newer.
    turn: tokio_watch::Sender<u64>,
    /// A reading (or the first opening) has the file.
    reading: bool,
    idle_since: Instant,
    /// When its page last asked after it.
    asked_at: Option<Instant>,
    ended: Option<(Ended, Instant)>,
}

/// The browsers watching movies.
#[derive(Default)]
pub struct MovieViewers {
    map: Mutex<HashMap<String, MovieViewer>>,
    /// Told when a reading gives the file back.
    returned: Notify,
    /// When viewers let their connection go, the last [`SETTLE_SECS`]'
    /// worth: a provider's count may still include them.
    released: Mutex<VecDeque<Instant>>,
    reaper: OnceLock<()>,
    /// Where a movie's file is, in place of the account's own answer: the
    /// tests' fixture servers.
    playables: Mutex<Option<Arc<Resolve>>>,
}

impl MovieViewers {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, MovieViewer>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The connections the viewers hold, as the sharing rules see them: one
    /// per viewer, with its rider; and with none for one that has ended
    /// while its reading still has the file (it is closing).
    pub fn conns(&self) -> Vec<Conn> {
        self.map()
            .values()
            .filter(|v| v.ended.is_none() || v.reading)
            .map(|v| Conn {
                feed: movie_feed(v.movie_id),
                riders: if v.ended.is_none() {
                    vec![Rider::viewer(v.rider)]
                } else {
                    Vec::new()
                },
            })
            .collect()
    }

    /// Viewers `device` holds now.
    pub fn count_for(&self, device: &str) -> usize {
        self.map()
            .values()
            .filter(|v| v.ended.is_none() && v.device == device)
            .count()
    }

    /// Viewers holding a place now.
    pub fn len(&self) -> usize {
        self.map().values().filter(|v| v.ended.is_none()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many viewers let their connection go within [`SETTLE_SECS`].
    pub fn released_recently(&self) -> usize {
        let mut r = self.released.lock().unwrap_or_else(|e| e.into_inner());
        let settle = Duration::from_secs(SETTLE_SECS);
        while r.front().is_some_and(|t| t.elapsed() >= settle) {
            r.pop_front();
        }
        r.len()
    }

    /// Drops a viewer's file for good.
    fn release(&self, up: HttpUpstream) {
        drop(up);
        self.note_released();
    }

    /// Counts a file let go of just now.
    fn note_released(&self) {
        self.released
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(Instant::now());
    }

    /// Ends `v` for `why`: its file let go, or its reading told to.
    fn finish(&self, v: &mut MovieViewer, why: Ended) {
        if v.ended.is_none() {
            v.ended = Some((why, Instant::now()));
        }
        if let Some(up) = v.up.take() {
            self.release(up);
        }
        v.turn.send_replace(u64::MAX);
    }

    /// Ends a viewer on `feed` (a movie's), saying another browser took its
    /// place ("Watch here anyway"). One, whichever: each has a connection of
    /// its own, and "anyway" may take any other browser's.
    pub fn end(&self, feed: i64) {
        let mut map = self.map();
        if let Some(v) = map
            .values_mut()
            .filter(|v| v.ended.is_none() && movie_feed(v.movie_id) == feed)
            .min_by_key(|v| v.idle_since)
        {
            self.finish(v, Ended::TakenOver);
        }
    }

    /// Viewer `id` of `device`, as its page asks after it: its movie, its
    /// rider and why it ended. A page that asks is still open, so its viewer
    /// is kept for [`ASKED_IDLE`] even when nothing reads from it: a browser
    /// that holds all it needs for now (a short film read to its end, a file
    /// read well ahead) keeps its place.
    fn asked(&self, device: &str, id: &str) -> Option<(i64, i64, Option<Ended>)> {
        let mut map = self.map();
        let v = map
            .get_mut(id)
            .filter(|v| v.device == device && !matches!(v.ended, Some((Ended::Stopped, _))))?;
        if v.ended.is_none() {
            v.asked_at = Some(Instant::now());
        }
        Some((v.movie_id, v.rider, v.ended.map(|(why, _)| why)))
    }

    /// Ends viewer `id` of `device`, if it is that device's. Its rider.
    pub(super) fn stop(&self, device: &str, id: &str) -> Option<i64> {
        let mut map = self.map();
        let v = map.get_mut(id).filter(|v| v.device == device)?;
        let rider = v.rider;
        self.finish(v, Ended::Stopped);
        let reading = v.reading;
        if !reading {
            map.remove(id);
        }
        Some(rider)
    }

    /// Ends the viewer whose rider is `rider`, saying another viewer took
    /// its place: exactly that one, though another device may watch the
    /// same movie.
    pub(super) fn end_rider(&self, rider: i64) {
        let mut map = self.map();
        if let Some(v) = map
            .values_mut()
            .find(|v| v.ended.is_none() && v.rider == rider)
        {
            self.finish(v, Ended::TakenOver);
        }
    }

    /// One of `device`'s movie viewers holding a connection among `conns`:
    /// the connection's feed, and the viewer's rider, which names it alone
    /// (another device may watch the same movie, on the same feed).
    pub(super) fn only_mine(&self, conns: &[Conn], device: &str) -> Option<(i64, i64)> {
        let map = self.map();
        let mine = |rider: i64| {
            map.values()
                .any(|v| v.rider == rider && v.ended.is_none() && v.device == device)
        };
        conns
            .iter()
            .filter(|c| c.riders.len() == 1)
            .map(|c| (c.feed, c.riders[0].id))
            .find(|&(_, r)| mine(r))
    }

    /// Lets go of viewers nobody reads, and forgets ended ones.
    fn reap(&self) {
        let mut map = self.map();
        let gone: Vec<String> = map
            .iter()
            .filter(|(_, v)| match v.ended {
                _ if v.reading => false,
                Some((Ended::Stopped, _)) => true,
                Some((_, at)) => at.elapsed() >= KEEP_ENDED,
                None => {
                    v.idle_since.elapsed() >= IDLE
                        && v.asked_at.is_none_or(|a| a.elapsed() >= ASKED_IDLE)
                }
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            if let Some(mut v) = map.remove(&id) {
                self.finish(&mut v, Ended::Stopped);
            }
        }
    }

    /// Answers [`Self::playable`] with `f` instead of the account.
    pub fn set_playables(&self, f: Arc<Resolve>) {
        *self.playables.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }

    /// Where movie `movie_id`'s file is, and how to ask for it: the
    /// account's answer, read afresh.
    fn playable(&self, movie_id: i64) -> Result<Playable> {
        let over = self
            .playables
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match over {
            Some(f) => f(movie_id),
            None => Source::load().and_then(|s| s.movie(movie_id)),
        }
    }

    #[cfg(test)]
    /// Viewer `id`'s file holds subtitle tracks `subs`.
    pub(super) fn set_subtitles_for_test(&self, id: &str, subs: Vec<SubtitleStream>) {
        if let Some(v) = self.map().get_mut(id) {
            v.subtitles = subs;
        }
    }

    #[cfg(test)]
    /// A viewer of `movie_id` for `device` holding no file, as the rules see
    /// one.
    pub(super) fn add_for_test(&self, device: &str, movie_id: i64, rider: i64) -> String {
        let id = new_id();
        self.map().insert(
            id.clone(),
            MovieViewer {
                device: device.into(),
                movie_id,
                rider,
                mode: VodMode::Direct,
                up: None,
                from: 0.0,
                audio: None,
                can: Can::default(),
                subtitles: Vec::new(),
                sounds: 0,
                cues: Cues::default(),
                turn: tokio_watch::channel(0).0,
                reading: false,
                idle_since: Instant::now(),
                asked_at: None,
                ended: None,
            },
        );
        id
    }
}

// ------------------------------------------------------------------- DTOs

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VodWatchRequest {
    movie_id: i64,
    #[serde(default)]
    anyway: bool,
    /// The viewer this one takes over from (a channel's or a movie's): it
    /// is stopped first, so its connection is free for this one.
    #[serde(default)]
    replaces: Option<String>,
    /// The sound to play: an index into the details' list, which is the
    /// file's own (the answer's) once the file is read.
    #[serde(default)]
    audio: Option<usize>,
    /// Where to start, in seconds from the start of the film: the
    /// remuxer's business (direct, the browser's player seeks itself); a
    /// stream request without `from` starts there.
    #[serde(default)]
    from: f64,
    #[serde(default)]
    can: CanDto,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TrackDto {
    index: usize,
    codec: String,
    lang: Option<String>,
    described: bool,
    impaired: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubtitleDto {
    /// What `/api/vod/subtitles/{viewer}/{index}` takes.
    index: usize,
    language: Option<String>,
    title: Option<String>,
    /// Text, which the page can show; pictures it cannot.
    text: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VodWatchDto {
    viewer: String,
    /// Where to read it: `/api/vod/stream/{viewer}`.
    stream: String,
    /// `direct` or `remux`.
    mode: &'static str,
    /// In seconds: the file's (remux), or the details' when they say.
    duration: Option<i64>,
    /// The sounds: the file's (remux), or the details' (direct). A track's
    /// `index` is what the watch's and the stream's `audio` take.
    audio: Vec<TrackDto>,
    audio_index: Option<usize>,
    /// The sound is being encoded to AAC on the way.
    converting: bool,
    /// The file's subtitles (remux; none direct).
    subtitles: Vec<SubtitleDto>,
    notice: Option<NoticeDto>,
}

/// The sounds the details list. Their codecs and languages are two lists,
/// which name the same tracks when they are as long as each other.
fn tracks(info: Option<&MovieInfo>) -> Vec<TrackDto> {
    let Some(i) = info else { return Vec::new() };
    let langs = (i.audio_languages.len() == i.audio_codecs.len()).then_some(&i.audio_languages);
    i.audio_codecs
        .iter()
        .enumerate()
        .map(|(index, codec)| TrackDto {
            index,
            codec: codec.to_ascii_lowercase(),
            lang: langs.and_then(|l| l.get(index).cloned()),
            described: false,
            impaired: false,
        })
        .collect()
}

/// The sounds the file holds, numbered as the answer lists them.
fn file_tracks(info: &MediaInfo) -> Vec<TrackDto> {
    info.audio
        .iter()
        .enumerate()
        .map(|(index, a)| TrackDto {
            index,
            codec: a.codec.to_ascii_lowercase(),
            lang: a.lang.clone(),
            described: false,
            impaired: false,
        })
        .collect()
}

fn subtitle_dtos(subs: &[SubtitleStream]) -> Vec<SubtitleDto> {
    subs.iter()
        .enumerate()
        .map(|(index, t)| SubtitleDto {
            index,
            language: t.lang.clone(),
            title: t.title.clone(),
            text: t.text,
        })
        .collect()
}

// --------------------------------------------------------------- handlers

pub fn routes() -> Router<Arc<Server>> {
    Router::new()
        .route("/api/vod/watch", post(start))
        .route("/api/vod/watch/{id}", get(status).delete(stop))
        .route("/api/vod/stream/{id}", get(stream))
        .route("/api/vod/subtitles/{id}/{index}", get(subtitles))
}

fn start_reaper(s: &Arc<Server>) {
    let weak: Weak<Server> = Arc::downgrade(s);
    s.vod.reaper.get_or_init(move || {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let Some(s) = weak.upgrade() else { return };
                s.vod.reap();
            }
        });
    });
}

/// What the catalogue says of a movie: its container and its length.
async fn movie_row(movie_id: i64) -> Result<Option<(Option<String>, Option<i64>)>> {
    tokio::task::spawn_blocking(move || {
        let Some(src) = db::with_db(|c| vod_store::movie_source(c, movie_id))? else {
            return Ok(None);
        };
        let length = reads::movies_by_ids(&[movie_id])?
            .into_iter()
            .next()
            .and_then(|m| m.duration_secs);
        Ok(Some((src.container, length)))
    })
    .await
    .map_err(|e| anyhow!("{e}"))?
}

/// Why the provider would not give the file, as live's "didn't start".
fn feed_refusal(f: Failure) -> Response {
    refused(
        StatusCode::BAD_GATEWAY,
        RefusalDto {
            error: format!("The movie didn't start: {}.", f.describe()),
            reason: "feed",
            anyway: f.is_limit(),
            slots: None,
            ending: Some(ending_of(f)),
            watch_instead: Vec::new(),
        },
    )
}

async fn start(
    State(s): State<Arc<Server>>,
    Extension(caller): Extension<Caller>,
    body: Result<Json<VodWatchRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(req)) = body else {
        return plain(
            StatusCode::BAD_REQUEST,
            "movie",
            "Not a request to watch a movie.",
        );
    };
    if !req.from.is_finite() || req.from < 0.0 {
        return plain(
            StatusCode::BAD_REQUEST,
            "movie",
            "from: seconds from the start of the movie.",
        );
    }
    let device = device_of(&caller);
    if let Some(old) = req.replaces.as_deref() {
        watch::let_go(&s, &device, old).await;
    }
    if let Err(why) = s
        .viewers
        .admit(&device, s.vod.count_for(&device), s.vod.len())
    {
        return plain(StatusCode::TOO_MANY_REQUESTS, "limit", why);
    }
    start_reaper(&s);

    // What it is, before any connection.
    let movie_id = req.movie_id;
    let (container, length) = match movie_row(movie_id).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            return plain(
                StatusCode::NOT_FOUND,
                "movie",
                "That movie is not in the catalogue any more.",
            )
        }
        Err(e) => {
            tracing::warn!("reading movie {movie_id}: {}", scrub(&format!("{e:#}")));
            return plain(
                StatusCode::INTERNAL_SERVER_ERROR,
                "movie",
                "The catalogue could not be read.",
            );
        }
    };
    // Unknown details (a provider that does not answer, a playlist's
    // movie) mean the remuxer, which reads what the file holds; the
    // failure is logged, scrubbed, where it happens.
    let info = vod_details::movie_info(movie_id).await.ok().flatten();
    let can = Can::from(req.can);
    let mut mode = vod_mode(container.as_deref(), info.as_ref(), &can);
    // A browser's player plays a file's first sound: another is the
    // remuxer's to pick.
    if req.audio.is_some_and(|a| a > 0) {
        mode = VodMode::Remux;
    }
    let playable = match s.vod.playable(movie_id) {
        Ok(p) => p,
        Err(e) => {
            return plain(
                StatusCode::NOT_FOUND,
                "movie",
                format!("Can't play this movie: {}", scrub(&format!("{e:#}"))),
            )
        }
    };

    // A connection of its own.
    if let Err(r) = watch::may_connect(&s, req.anyway, &device).await {
        return *r;
    }
    let mut up = match HttpUpstream::new(playable) {
        Ok(u) => u,
        Err(_) => return feed_refusal(Failure::Unreachable),
    };
    let rider = s.viewers.rider_id();
    let id = new_id();
    let (turn_tx, mut turn) = tokio_watch::channel(0);
    // Counted among the service's connections while it opens.
    s.vod.map().insert(
        id.clone(),
        MovieViewer {
            device: device.clone(),
            movie_id,
            rider,
            mode,
            up: None,
            from: req.from,
            audio: req.audio,
            can,
            subtitles: Vec::new(),
            sounds: 0,
            cues: Cues::default(),
            turn: turn_tx,
            reading: true,
            idle_since: Instant::now(),
            asked_at: None,
            ended: None,
        },
    );
    // On a task of its own, so that a browser giving up on the answer
    // leaves a viewer the reaper lets go, not one opening for ever; and
    // given up at once when the viewer is stopped or taken over meanwhile,
    // so that the one replacing it never meets it at the provider. A file
    // for the remuxer is read on, to learn what it holds; its response is
    // let go then.
    let opening = tokio::spawn({
        let s = Arc::clone(&s);
        let id = id.clone();
        async move {
            let opened = tokio::select! {
                o = up.open(0, None) => Some(match o {
                    Ok(span) if span.beyond => Err(Failure::NotVideo),
                    Ok(_) => Ok(None),
                    Err(f) => Err(f),
                }),
                _ = superseded(&mut turn, 0) => None,
            };
            let (up, opened) = match opened {
                Some(Ok(_)) if mode == VodMode::Remux => {
                    let (stop, stopped) = tokio_watch::channel(false);
                    let learning = learn(up, stopped);
                    tokio::pin!(learning);
                    tokio::select! {
                        (up, learnt) = &mut learning => (up, Some(learnt.map(Some))),
                        _ = superseded(&mut turn, 0) => {
                            let _ = stop.send(true);
                            ((&mut learning).await.0, None)
                        }
                    }
                }
                o => (Some(up), o),
            };
            let mut up = up;
            let mut map = s.vod.map();
            if let (Some(v), Some(Ok(media))) =
                (map.get_mut(&id).filter(|v| v.ended.is_none()), &opened)
            {
                if let Some(up) = up.take() {
                    if let Some(m) = media {
                        v.subtitles = m.subtitles.clone();
                        v.sounds = m.audio.len();
                    }
                    v.up = Some(up);
                    v.reading = false;
                    v.idle_since = Instant::now();
                    return Ok(media.clone());
                }
            }
            map.remove(&id);
            match up {
                Some(up) => s.vod.release(up),
                // Let go of with the remuxer's thread.
                None => s.vod.note_released(),
            }
            // `None`: taken over, or stopped, while it opened.
            Err(opened.and_then(Result::err))
        }
    });
    let media = match opening.await {
        Ok(Ok(media)) => media,
        Ok(Err(Some(f))) => return feed_refusal(f),
        Ok(Err(None)) | Err(_) => return plain(StatusCode::GONE, "feed", "This stream has ended."),
    };

    // What the answer lists: the file's, once read; the details' otherwise.
    let (audio, audio_index, converting, duration, subtitles) = match &media {
        Some(m) => {
            let audio_index = req
                .audio
                .filter(|&a| a < m.audio.len())
                .or((!m.audio.is_empty()).then_some(0));
            let plan = match plan_for(m, audio_index, &can) {
                Ok(p) => p,
                Err(why) => {
                    s.vod.stop(&device, &id);
                    return plain(StatusCode::UNPROCESSABLE_ENTITY, "notPlayable", why);
                }
            };
            let audio_index = plan.audio.and(audio_index);
            if let Some(v) = s.vod.map().get_mut(&id) {
                v.audio = audio_index;
            }
            (
                file_tracks(m),
                audio_index,
                plan.encode,
                m.duration.map(|d| d.round() as i64),
                subtitle_dtos(&m.subtitles),
            )
        }
        None => {
            let audio = tracks(info.as_ref());
            let audio_index = (!audio.is_empty()).then_some(0);
            (audio, audio_index, false, None, Vec::new())
        }
    };

    let notice = watch::notice_for(&s, movie_feed(movie_id), rider).await;
    let dto = VodWatchDto {
        viewer: id.clone(),
        stream: format!("/api/vod/stream/{id}"),
        mode: match mode {
            VodMode::Direct => "direct",
            VodMode::Remux => "remux",
        },
        duration: duration
            .or(info.as_ref().and_then(|i| i.runtime))
            .or(length),
        audio,
        audio_index,
        converting,
        subtitles,
        notice,
    };
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(dto),
    )
        .into_response()
}

async fn status(
    State(s): State<Arc<Server>>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> Response {
    let found = s.vod.asked(&device_of(&caller), &id);
    let Some((movie_id, rider, ended)) = found else {
        return plain(StatusCode::NOT_FOUND, "movie", "No such viewer.");
    };
    let dropped = EndingDto {
        kind: "dropped",
        status: None,
        limit: false,
    };
    let dto = match ended {
        Some(Ended::Feed(f)) => StatusDto {
            ended: Some(ending_of(f)),
            taken_over: false,
            notice: None,
        },
        Some(_) => StatusDto {
            ended: Some(dropped),
            taken_over: true,
            notice: None,
        },
        None => StatusDto {
            ended: None,
            taken_over: false,
            notice: watch::notice_for(&s, movie_feed(movie_id), rider).await,
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
    s.vod.stop(&device_of(&caller), &id);
    StatusCode::NO_CONTENT.into_response()
}

/// The one range a browser's player asks for (`bytes=first-` or
/// `bytes=first-last`); anything else is read as the whole file.
fn asked_range(h: &HeaderMap) -> Option<(u64, Option<u64>)> {
    let v = h.get(header::RANGE)?.to_str().ok()?.trim();
    let (first, last) = v.strip_prefix("bytes=")?.split_once('-')?;
    let first: u64 = first.trim().parse().ok()?;
    let last = match last.trim() {
        "" => None,
        l => Some(l.parse::<u64>().ok().filter(|&l| l >= first)?),
    };
    Some((first, last))
}

/// A remux's request: where in the film to start (seconds), and the sound
/// (an index into the watch answer's list). The watch's, unsaid.
#[derive(Debug, Deserialize)]
struct StreamQuery {
    from: Option<f64>,
    audio: Option<usize>,
}

/// What a remux needs from its viewer.
struct RemuxAsk {
    from: f64,
    audio: Option<usize>,
    can: Can,
}

async fn stream(
    State(s): State<Arc<Server>>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    query: Result<Query<StreamQuery>, QueryRejection>,
    req: Request,
) -> Response {
    // Answering a HEAD would ask the provider for the file, and end the
    // reading the page has: nothing a player needs.
    if req.method() == Method::HEAD {
        return (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "GET")]).into_response();
    }
    if watch::from_another_site(req.headers()) {
        return plain(
            StatusCode::FORBIDDEN,
            "movie",
            "refused: the request came from another site",
        );
    }
    let asked = match query {
        Ok(Query(q)) if q.from.is_none_or(|f| f.is_finite() && f >= 0.0) => q,
        _ => {
            return plain(
                StatusCode::BAD_REQUEST,
                "movie",
                "from: seconds from the start of the movie; audio: a sound's index.",
            )
        }
    };
    let device = device_of(&caller);
    let range = asked_range(req.headers());

    // Take the file: from the viewer, or from an older reading, which is
    // told to stop and gives it back, its response dropped.
    let deadline = Instant::now() + Duration::from_secs(3);
    let (mut up, turn, mine, remuxing) = loop {
        let waiter = s.vod.returned.notified();
        {
            let mut map = s.vod.map();
            let Some(v) = map
                .get_mut(&id)
                .filter(|v| v.device == device && !matches!(v.ended, Some((Ended::Stopped, _))))
            else {
                return plain(StatusCode::NOT_FOUND, "movie", "No such viewer.");
            };
            if v.ended.is_some() {
                return plain(StatusCode::GONE, "feed", "This stream has ended.");
            }
            // Refused before it ends the reading there is.
            if v.mode == VodMode::Remux && asked.audio.is_some_and(|a| a >= v.sounds) {
                return plain(
                    StatusCode::BAD_REQUEST,
                    "movie",
                    "audio: the index of one of the movie's sounds.",
                );
            }
            let next = v.turn.borrow().wrapping_add(1);
            v.turn.send_replace(next);
            if let Some(up) = v.up.take() {
                v.reading = true;
                let remuxing = (v.mode == VodMode::Remux).then(|| {
                    // The subtitles the page reads are this remux's, and the
                    // sound chosen stays chosen.
                    v.cues = Cues::new(next);
                    v.audio = asked.audio.or(v.audio);
                    RemuxAsk {
                        from: asked.from.unwrap_or(v.from),
                        audio: v.audio,
                        can: v.can,
                    }
                });
                break (up, v.turn.subscribe(), next, remuxing);
            }
        }
        if Instant::now() >= deadline {
            return plain(
                StatusCode::CONFLICT,
                "movie",
                "This stream is being read elsewhere.",
            );
        }
        let _ = tokio::time::timeout(Duration::from_millis(500), waiter).await;
    };

    if let Some(ask) = remuxing {
        return remux_stream(s, id, up, turn, mine, ask).await;
    }

    // The provider asked, then the reading, on a task of their own: a
    // browser that gives up on this request while the provider answers
    // must not take the viewer's file with it.
    let (first, last) = range.unwrap_or((0, None));
    let (tx, rx) = mpsc::channel::<Bytes>(16);
    let (answer, answered) = oneshot::channel();
    let server = Arc::clone(&s);
    let reader_id = id.clone();
    tokio::spawn(async move {
        let mut turn = turn;
        // A newer request (or a stop) while the provider answers: this one
        // is given up at once, its request dropped before the next is sent.
        let opened = tokio::select! {
            o = up.open(first, last) => o.map_err(Some),
            _ = superseded(&mut turn, mine) => Err(None),
        };
        match opened {
            Err(None) => {
                give_back(&server, &reader_id, up);
                let _ = answer.send(Err(None));
            }
            Ok(span) if !span.beyond => {
                let _ = answer.send(Ok(span));
                let up = pass_on(up, &tx, turn, mine).await;
                give_back(&server, &reader_id, up);
            }
            Ok(span) => {
                give_back(&server, &reader_id, up);
                let _ = answer.send(Ok(span));
            }
            Err(Some(f)) => {
                feed_failed(&server, &reader_id, up, f);
                let _ = answer.send(Err(Some(f)));
            }
        }
    });
    let span = match answered.await {
        Ok(Ok(span)) => span,
        Ok(Err(Some(f))) => return feed_refusal(f),
        Ok(Err(None)) => {
            return plain(
                StatusCode::CONFLICT,
                "movie",
                "A newer request took this stream.",
            )
        }
        Err(_) => return feed_refusal(Failure::Unreachable),
    };
    if span.beyond {
        let mut r = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
        if let Some(total) = span.total {
            if let Ok(v) = format!("bytes */{total}").parse() {
                r.headers_mut().insert(header::CONTENT_RANGE, v);
            }
        }
        return r;
    }

    // A part, as the provider gave it, when one was asked for; the whole
    // file from its start otherwise.
    let partial = range.is_some() && span.partial;
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "video/mp4".parse().unwrap());
    headers.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    if let Some(n) = span.length {
        headers.insert(header::CONTENT_LENGTH, n.into());
    }
    if let Some(v) = span
        .content_range()
        .filter(|_| partial)
        .and_then(|v| v.parse().ok())
    {
        headers.insert(header::CONTENT_RANGE, v);
    }
    let status = if partial {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    (status, headers, Body::from_stream(Chunks(rx))).into_response()
}

/// The provider will not give the file any more: the viewer has ended, and
/// its page is told why.
fn feed_failed(s: &Server, id: &str, up: HttpUpstream, f: Failure) {
    let mut map = s.vod.map();
    match map.get_mut(id) {
        Some(v) => {
            v.reading = false;
            v.up = Some(up);
            s.vod.finish(v, Ended::Feed(f));
        }
        None => s.vod.release(up),
    }
    drop(map);
    s.vod.returned.notify_waiters();
}

/// How a remux began.
enum Began {
    /// Its output starts this many seconds into the film.
    At(f64),
    /// A newer request (or a stop) came first.
    Superseded,
    /// The provider would not give the file.
    Feed(Failure),
    /// No browser here plays it, and why.
    NotPlayable(String),
    /// It ended before its output started.
    Failed,
    /// Its first keyframe took longer than [`START_WAIT`].
    TooSlow,
}

/// How long a remux has to reach its first keyframe.
const START_WAIT: Duration = Duration::from_secs(60);

/// A remux's answer: MPEG-TS from the film's keyframe at or before `from`,
/// once the remuxer has found it, named in `X-Start`.
async fn remux_stream(
    s: Arc<Server>,
    id: String,
    up: HttpUpstream,
    turn: tokio_watch::Receiver<u64>,
    mine: u64,
    ask: RemuxAsk,
) -> Response {
    let (tx, rx) = mpsc::channel::<Bytes>(16);
    let (answer, answered) = oneshot::channel();
    tokio::spawn(remux(s, id, up, turn, mine, ask, tx, answer));
    let at = match answered.await {
        Ok(Began::At(at)) => at,
        Ok(Began::Superseded) => {
            return plain(
                StatusCode::CONFLICT,
                "movie",
                "A newer request took this stream.",
            )
        }
        Ok(Began::Feed(f)) => return feed_refusal(f),
        Ok(Began::NotPlayable(why)) => {
            return plain(StatusCode::UNPROCESSABLE_ENTITY, "notPlayable", why)
        }
        Ok(Began::TooSlow) => {
            return plain(
                StatusCode::GATEWAY_TIMEOUT,
                "feed",
                "The movie took too long to start from there.",
            )
        }
        Ok(Began::Failed) | Err(_) => {
            return plain(
                StatusCode::BAD_GATEWAY,
                "feed",
                "The movie could not be converted from there.",
            )
        }
    };
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "video/mp2t".parse().unwrap());
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    if let Ok(v) = format!("{at:.3}").parse() {
        headers.insert("x-start", v);
    }
    (StatusCode::OK, headers, Body::from_stream(Chunks(rx))).into_response()
}

/// A remux from `ask.from`, on a task of its own: the remuxer started on the
/// viewer's file, `answer` told how it began, its output passed to `tx`
/// and its cues kept for the page until it ends, the browser leaves, or a
/// newer reading (or a stop) comes; then the remuxer is stopped, and the
/// file given back once its thread lets go of it.
#[allow(clippy::too_many_arguments)]
async fn remux(
    s: Arc<Server>,
    id: String,
    mut up: HttpUpstream,
    mut turn: tokio_watch::Receiver<u64>,
    mine: u64,
    ask: RemuxAsk,
    tx: mpsc::Sender<Bytes>,
    answer: oneshot::Sender<Began>,
) {
    let (stop, stopped) = tokio_watch::channel(false);
    up.interrupt_on(Some(stopped));
    let (lent, back) = Lent::new(up);
    let refused = Arc::new(Mutex::new(None::<String>));
    let why = Arc::clone(&refused);
    let RemuxAsk { from, audio, can } = ask;
    let (_, vod) = vodmux::spawn(Box::new(lent), from, move |info| {
        plan_for(info, audio, &can).inspect_err(|e| {
            *why.lock().unwrap_or_else(|e| e.into_inner()) = Some(e.clone());
        })
    });
    let Vodmux {
        mut output,
        mut start,
        cues,
    } = vod;
    let mut too_slow = false;
    let at = tokio::select! {
        at = &mut start => at.ok(),
        _ = superseded(&mut turn, mine) => None,
        _ = tx.closed() => None,
        _ = tokio::time::sleep(START_WAIT) => {
            too_slow = true;
            None
        }
    };
    let mut answer = Some(answer);
    if let Some(at) = at {
        tokio::spawn(keep_cues(Arc::clone(&s), id.clone(), mine, at, cues));
        if answer.take().is_some_and(|a| a.send(Began::At(at)).is_ok()) {
            pass_ts(&mut output, &tx, &mut turn, mine).await;
        }
    }
    // Stopped at its next read or write, whichever comes first; its output
    // gone first, so that it does not count the stop as a failure.
    drop(output);
    let _ = stop.send(true);
    let Ok(mut up) = back.await else {
        // Not to be: a lent file always comes back. Should it not, the
        // viewer has no file to go on with.
        s.vod.note_released();
        if let Some(v) = s.vod.map().get_mut(&id) {
            v.reading = false;
            s.vod.finish(v, Ended::Feed(Failure::Unreachable));
        }
        s.vod.returned.notify_waiters();
        if let Some(a) = answer {
            let _ = a.send(Began::Failed);
        }
        return;
    };
    up.interrupt_on(None);
    let failure = up.take_failure();
    let Some(answer) = answer else {
        give_back(&s, &id, up);
        return;
    };
    // It never started: why.
    if let Some(f) = failure {
        feed_failed(&s, &id, up, f);
        let _ = answer.send(Began::Feed(f));
        return;
    }
    let began = match refused.lock().unwrap_or_else(|e| e.into_inner()).take() {
        Some(why) => Began::NotPlayable(why),
        None if *turn.borrow() != mine => Began::Superseded,
        None if too_slow => Began::TooSlow,
        None => Began::Failed,
    };
    give_back(&s, &id, up);
    let _ = answer.send(began);
}

/// A remux's output, to the browser, until it ends, the browser leaves, or
/// a newer reading (or a stop) comes.
async fn pass_ts(
    output: &mut mpsc::Receiver<Bytes>,
    tx: &mpsc::Sender<Bytes>,
    turn: &mut tokio_watch::Receiver<u64>,
    mine: u64,
) {
    loop {
        if *turn.borrow() != mine {
            break;
        }
        tokio::select! {
            b = output.recv() => match b {
                // A browser that stopped reading must not keep a newer
                // reading (or a stop) waiting.
                Some(b) => tokio::select! {
                    r = tx.send(b) => if r.is_err() { break },
                    r = turn.changed() => if r.is_err() { break },
                },
                None => break,
            },
            r = turn.changed() => if r.is_err() { break },
            _ = tx.closed() => break,
        }
    }
}

/// A remux's cues, which began `at` seconds into the film, kept for the
/// page in the film's time, while the remux is the viewer's latest.
async fn keep_cues(
    s: Arc<Server>,
    id: String,
    mine: u64,
    at: f64,
    mut cues: mpsc::UnboundedReceiver<Cue>,
) {
    while let Some(c) = cues.recv().await {
        let mut map = s.vod.map();
        let Some(v) = map.get_mut(&id).filter(|v| v.cues.turn == mine) else {
            return;
        };
        v.cues.push(Cue {
            start: at + c.start,
            end: at + c.end,
            ..c
        });
    }
}

/// A viewer's file lent to the remuxer, which reads it on a thread of its
/// own: handed back when the remuxer lets go of it, as its thread ends.
struct Lent {
    up: Option<HttpUpstream>,
    back: Option<oneshot::Sender<HttpUpstream>>,
}

impl Lent {
    /// The remuxer reads the file from its start, wherever an earlier
    /// reading left it.
    fn new(mut up: HttpUpstream) -> (Self, oneshot::Receiver<HttpUpstream>) {
        let _ = Upstream::seek(&mut up, 0);
        let (tx, rx) = oneshot::channel();
        (
            Self {
                up: Some(up),
                back: Some(tx),
            },
            rx,
        )
    }
}

impl Drop for Lent {
    fn drop(&mut self) {
        if let (Some(up), Some(back)) = (self.up.take(), self.back.take()) {
            let _ = back.send(up);
        }
    }
}

impl Upstream for Lent {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.up.as_mut().map_or(Ok(0), |u| u.read(buf))
    }
    fn seek(&mut self, pos: u64) -> std::io::Result<()> {
        self.up.as_mut().map_or(Ok(()), |u| u.seek(pos))
    }
    fn size(&self) -> Option<u64> {
        self.up.as_ref().and_then(Upstream::size)
    }
}

/// What the file holds, read by the remuxer from the response `up` has
/// open, which is then let go; given up when `stop` says so. The file,
/// unless the remuxer's thread let go of it.
async fn learn(
    mut up: HttpUpstream,
    stop: tokio_watch::Receiver<bool>,
) -> (Option<HttpUpstream>, Result<MediaInfo, Failure>) {
    up.interrupt_on(Some(stop.clone()));
    let (lent, back) = Lent::new(up);
    let (seen, learnt) = oneshot::channel();
    // The plan is asked once the streams are listed, and says no: nothing
    // is written.
    let (said, vod) = vodmux::spawn(Box::new(lent), 0.0, move |info| {
        let _ = seen.send(info.clone());
        Err(String::new())
    });
    if let Ok(Err(why)) = said.await {
        if !why.is_empty() && !*stop.borrow() {
            tracing::warn!("reading a movie's file for a browser: {why}");
        }
    }
    drop(vod);
    let mut up = back.await.ok();
    let failure = up.as_mut().and_then(|u| {
        u.interrupt_on(None);
        u.close();
        u.take_failure()
    });
    let learnt = match (learnt.await, &up) {
        (Ok(info), Some(_)) => Ok(info),
        _ => Err(failure.unwrap_or(Failure::NotVideo)),
    };
    (up, learnt)
}

/// A subtitle request: cues ending after this, in seconds into the film.
#[derive(Debug, Deserialize)]
struct CuesQuery {
    after: Option<f64>,
}

/// A viewer's subtitle track `index` (as the watch answer lists them): the
/// cues its current remux has read that end after `after`, as WebVTT, in
/// the film's time.
async fn subtitles(
    State(s): State<Arc<Server>>,
    Extension(caller): Extension<Caller>,
    Path((id, index)): Path<(String, String)>,
    query: Result<Query<CuesQuery>, QueryRejection>,
    headers: HeaderMap,
) -> Response {
    if watch::from_another_site(&headers) {
        return plain(
            StatusCode::FORBIDDEN,
            "movie",
            "refused: the request came from another site",
        );
    }
    let after = match query {
        Ok(Query(q)) if q.after.is_none_or(f64::is_finite) => {
            q.after.unwrap_or(f64::NEG_INFINITY)
        }
        _ => {
            return plain(
                StatusCode::BAD_REQUEST,
                "movie",
                "after: seconds from the start of the movie.",
            )
        }
    };
    let device = device_of(&caller);
    let map = s.vod.map();
    let Some(v) = map
        .get(&id)
        .filter(|v| v.device == device && !matches!(v.ended, Some((Ended::Stopped, _))))
    else {
        return plain(StatusCode::NOT_FOUND, "movie", "No such viewer.");
    };
    if v.ended.is_some() {
        return plain(StatusCode::GONE, "feed", "This stream has ended.");
    }
    let Some(track) = index.parse::<usize>().ok().and_then(|i| v.subtitles.get(i)) else {
        return plain(StatusCode::NOT_FOUND, "movie", "No such subtitles.");
    };
    if !track.text {
        return plain(
            StatusCode::NOT_FOUND,
            "movie",
            "These subtitles are pictures, which a browser can’t show.",
        );
    }
    let doc = v.cues.webvtt(track.index, after);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/vtt; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        doc,
    )
        .into_response()
}

/// The file back to its viewer after a reading, its response let go; or
/// let go of for good when the viewer has ended.
fn give_back(s: &Server, id: &str, mut up: HttpUpstream) {
    up.close();
    let mut map = s.vod.map();
    match map.get_mut(id) {
        Some(v) if v.ended.is_none() => {
            v.up = Some(up);
            v.reading = false;
            v.idle_since = Instant::now();
        }
        Some(v) => {
            v.reading = false;
            s.vod.release(up);
            if matches!(v.ended, Some((Ended::Stopped, _))) {
                map.remove(id);
            }
        }
        None => s.vod.release(up),
    }
    drop(map);
    s.vod.returned.notify_waiters();
}

/// Waits until a newer reading (or a stop) than `mine` has come, or the
/// viewer is gone.
async fn superseded(turn: &mut tokio_watch::Receiver<u64>, mine: u64) {
    while *turn.borrow_and_update() == mine {
        if turn.changed().await.is_err() {
            return;
        }
    }
}

/// The open response, to the browser, until it ends, the browser leaves, or
/// a newer reading (or a stop) comes. The file, its response dropped.
async fn pass_on(
    mut up: HttpUpstream,
    tx: &mpsc::Sender<Bytes>,
    mut turn: tokio_watch::Receiver<u64>,
    mine: u64,
) -> HttpUpstream {
    loop {
        if *turn.borrow() != mine {
            break;
        }
        tokio::select! {
            c = up.chunk() => match c {
                // A browser that stopped reading must not keep a newer
                // reading (or a stop) waiting.
                Ok(Some(b)) => tokio::select! {
                    r = tx.send(b) => if r.is_err() { break },
                    r = turn.changed() => if r.is_err() { break },
                },
                // The end of what was asked, or the provider dropped it: the
                // browser's player asks again from where it is.
                Ok(None) | Err(_) => break,
            },
            r = turn.changed() => if r.is_err() { break },
            _ = tx.closed() => break,
        }
    }
    up.close();
    up
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::budget::Slots;
    use crate::recording::watch::{decide, Decision, Refusal};

    fn details(video: &str, audio: &[&str]) -> MovieInfo {
        MovieInfo {
            video_codec: Some(video.into()),
            audio_codecs: audio.iter().map(|a| a.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_movie_feed_never_names_a_channel() {
        assert_eq!(movie_feed(7), -7);
        assert_eq!(movie_of(-7), Some(7));
        assert_eq!(movie_of(7), None);
        assert_eq!(movie_of(0), None);
        assert_eq!(movie_of(i64::MIN), None);
    }

    #[test]
    fn direct_only_for_mp4_that_browsers_play() {
        let can = Can::default();
        let h264_aac = details("h264", &["aac"]);
        assert_eq!(
            vod_mode(Some("mp4"), Some(&h264_aac), &can),
            VodMode::Direct
        );
        assert_eq!(
            vod_mode(Some("M4V"), Some(&h264_aac), &can),
            VodMode::Direct
        );
        assert_eq!(
            vod_mode(Some("mov"), Some(&details("H264", &["mp3"])), &can),
            VodMode::Direct
        );
        assert_eq!(
            vod_mode(Some("mp4"), Some(&details("h264", &[])), &can),
            VodMode::Direct
        );
        assert_eq!(vod_mode(Some("mkv"), Some(&h264_aac), &can), VodMode::Remux);
        assert_eq!(vod_mode(None, Some(&h264_aac), &can), VodMode::Remux);
        assert_eq!(
            vod_mode(Some("mp4"), Some(&details("h264", &["aac", "ac3"])), &can),
            VodMode::Remux
        );
        assert_eq!(vod_mode(Some("mp4"), None, &can), VodMode::Remux);
        assert_eq!(
            vod_mode(Some("mp4"), Some(&MovieInfo::default()), &can),
            VodMode::Remux,
            "no video codec said"
        );
        assert_eq!(
            vod_mode(Some("mp4"), Some(&details("hevc", &["aac"])), &can),
            VodMode::Remux
        );
        // Subtitles are shown from the remuxer's read.
        let subtitled = MovieInfo {
            subtitle_languages: vec!["eng".into()],
            ..h264_aac.clone()
        };
        assert_eq!(
            vod_mode(Some("mp4"), Some(&subtitled), &can),
            VodMode::Remux
        );
    }

    fn rec(id: i64, feed: i64) -> Conn {
        let t = crate::recording::now();
        Conn {
            feed,
            riders: vec![Rider {
                id,
                stop: t + 3600,
                until: t + 3900,
            }],
        }
    }

    fn movie(rider: i64, id: i64) -> Conn {
        Conn {
            feed: movie_feed(id),
            riders: vec![Rider::viewer(rider)],
        }
    }

    #[test]
    fn a_movie_viewer_is_one_of_the_services_connections() {
        // A recording on 5 and a browser watching movie 9: the account's
        // two connections are both the service's.
        let own = [rec(1, 5), movie(-1, 9)];
        let full = Slots::Full { active: 2, max: 2 };
        assert_eq!(
            decide(full, &own, 0, false),
            Decision::Refuse(Refusal::Viewers { feeds: vec![-9] }),
            "the movie's browser is named, and \"anyway\" offered"
        );
        assert_eq!(decide(full, &own, 0, true), Decision::TakeOver(-9));
        // Were the movie's connection not counted as the service's, it would
        // look like another device's, and with a recording at stake not
        // even "anyway" could connect.
        assert!(matches!(
            decide(full, &own[..1], 0, true),
            Decision::Refuse(Refusal::Recording { .. })
        ));
    }

    #[test]
    fn anyway_takes_a_movie_viewers_place_never_a_recordings() {
        let full = Slots::Full { active: 1, max: 1 };
        assert_eq!(
            decide(full, &[movie(-1, 9)], 0, true),
            Decision::TakeOver(-9)
        );
        assert_eq!(
            decide(full, &[rec(1, 5)], 0, true),
            Decision::Refuse(Refusal::Recording {
                feeds: vec![5],
                recordings: vec![1]
            })
        );
    }

    #[test]
    fn a_browsers_range_is_one_span_or_none() {
        let h = |v: &'static str| {
            let mut h = HeaderMap::new();
            h.insert(header::RANGE, header::HeaderValue::from_static(v));
            h
        };
        assert_eq!(asked_range(&h("bytes=0-")), Some((0, None)));
        assert_eq!(asked_range(&h("bytes=500-999")), Some((500, Some(999))));
        assert_eq!(asked_range(&h("bytes=500-10")), None);
        assert_eq!(asked_range(&h("bytes=-500")), None);
        assert_eq!(asked_range(&h("bytes=0-1, 5-9")), None);
        assert_eq!(asked_range(&HeaderMap::new()), None);
    }

    fn file(video: Option<&str>, audio: &[(usize, &str)]) -> MediaInfo {
        MediaInfo {
            duration: Some(20.0),
            video_codec: video.map(Into::into),
            audio: audio
                .iter()
                .map(|&(index, codec)| vodmux::AudioStream {
                    index,
                    codec: codec.into(),
                    lang: None,
                    channels: Some(2),
                })
                .collect(),
            subtitles: Vec::new(),
        }
    }

    #[test]
    fn the_asked_for_audio_is_used_and_converted_when_the_browser_lacks_it() {
        let info = file(Some("h264"), &[(1, "aac"), (2, "ac3")]);
        let can = Can::default();
        assert_eq!(
            remux_plan(&info, Some(2), &can),
            Ok(VodPlan {
                audio: Some(2),
                encode: true
            })
        );
        let ac3 = Can {
            ac3: true,
            ..Can::default()
        };
        assert_eq!(
            remux_plan(&info, Some(2), &ac3),
            Ok(VodPlan {
                audio: Some(2),
                encode: false
            })
        );
        // Unsaid, or not in the file: the first.
        for want in [None, Some(7)] {
            assert_eq!(remux_plan(&info, want, &can).unwrap().audio, Some(1));
        }
        // Numbered as the answer lists them, for the page.
        if can_encode_aac() {
            assert_eq!(plan_for(&info, Some(1), &can).unwrap().audio, Some(2));
        }
    }

    #[test]
    fn aac_is_passed_through() {
        let info = file(Some("h264"), &[(1, "aac")]);
        assert_eq!(
            remux_plan(&info, None, &Can::default()),
            Ok(VodPlan {
                audio: Some(1),
                encode: false
            })
        );
    }

    #[test]
    fn a_film_without_sound_plays_silent() {
        assert_eq!(
            remux_plan(&file(Some("h264"), &[]), Some(1), &Can::default()),
            Ok(VodPlan {
                audio: None,
                encode: false
            })
        );
    }

    #[test]
    fn mpeg2_video_is_refused() {
        let why = remux_plan(&file(Some("mpeg2video"), &[(1, "mp2")]), None, &Can::default())
            .unwrap_err();
        assert_eq!(
            why,
            "Its video is MPEG-2, which browsers can’t play. Watch it in the zapper app."
        );
        let hevc = file(Some("hevc"), &[(1, "aac")]);
        assert!(remux_plan(&hevc, None, &Can::default())
            .unwrap_err()
            .contains("this browser"));
        let can = Can {
            hevc: true,
            ..Can::default()
        };
        assert!(remux_plan(&hevc, None, &can).is_ok());
        assert!(remux_plan(&file(None, &[]), None, &can).is_err());
        assert!(remux_plan(&file(None, &[(0, "aac")]), None, &can).is_ok());
    }

    fn cue(track: usize, start: f64, text: &str) -> Cue {
        Cue {
            track,
            start,
            end: start + 2.0,
            text: text.into(),
        }
    }

    #[test]
    fn a_tracks_cues_are_kept_in_bounds_and_read_after_a_time() {
        let mut c = Cues::new(3);
        c.push(cue(2, 1.0, "one"));
        c.push(cue(2, 12.0, "two"));
        c.push(cue(4, 5.0, "elsewhere"));
        assert_eq!(
            c.webvtt(2, f64::NEG_INFINITY),
            "WEBVTT\n\n00:00:01.000 --> 00:00:03.000\none\n\n00:00:12.000 --> 00:00:14.000\ntwo\n\n"
        );
        assert_eq!(
            c.webvtt(2, 3.0),
            "WEBVTT\n\n00:00:12.000 --> 00:00:14.000\ntwo\n\n",
            "a cue that has ended by then is not sent"
        );
        assert_eq!(c.webvtt(9, 0.0), "WEBVTT\n\n");
        // Far behind the newest: let go.
        c.push(cue(2, 12.0 + CUES_BEHIND, "much later"));
        assert_eq!(c.tracks[&2].len(), 2, "{:?}", c.tracks[&2]);
        assert_eq!(c.tracks[&2][0].text, "two");
        for i in 0..MAX_CUES + 10 {
            c.push(cue(5, 700.0 + i as f64 / 10.0, "busy"));
        }
        assert_eq!(c.tracks[&5].len(), MAX_CUES);

        // All tracks together: the earliest cues go first, whichever track.
        let mut c = Cues::new(4);
        let tracks = MAX_CUES_TOTAL / MAX_CUES + 2;
        for i in 0..MAX_CUES {
            for t in 0..tracks {
                c.push(cue(t, i as f64 / 10.0, "many"));
            }
        }
        let kept: usize = c.tracks.values().map(VecDeque::len).sum();
        assert_eq!(kept, MAX_CUES_TOTAL);
        let earliest = c.tracks.values().map(|q| q[0].start).fold(f64::MAX, f64::min);
        let latest = c.tracks.values().map(|q| q[q.len() - 1].start).fold(0.0, f64::max);
        assert!(earliest > 0.0 && latest == (MAX_CUES - 1) as f64 / 10.0, "{earliest} {latest}");
    }

    #[test]
    fn a_viewer_its_page_asks_after_is_not_let_go_as_idle() {
        let vod = MovieViewers::default();
        let asked = vod.add_for_test("dev", 9, 1);
        let slow = vod.add_for_test("dev", 7, 3);
        let gone = vod.add_for_test("dev", 6, 4);
        let quiet = vod.add_for_test("dev", 8, 2);
        assert_eq!(
            vod.asked("dev", &asked).map(|(m, r, e)| (m, r, e.is_none())),
            Some((9, 1, true))
        );
        assert_eq!(vod.asked("other", &quiet), None, "another device's");
        let ago = |secs: u64| Instant::now().checked_sub(Duration::from_secs(secs));
        {
            let mut map = vod.map();
            // Every browser holds all it needs, and reads nothing for a while.
            for v in map.values_mut() {
                v.idle_since = ago(IDLE.as_secs() + 1).unwrap();
            }
            // A background tab's page asks about once a minute; a closed
            // page's last ask is longer ago than that.
            map.get_mut(&slow).unwrap().asked_at = ago(60);
            map.get_mut(&gone).unwrap().asked_at = ago(ASKED_IDLE.as_secs() + 1);
        }
        vod.reap();
        let map = vod.map();
        assert!(map.contains_key(&asked), "its page is still there");
        assert!(map.contains_key(&slow), "a background tab's is kept");
        assert!(!map.contains_key(&gone), "a page that stopped asking goes");
        assert!(!map.contains_key(&quiet), "one never asked after goes");
    }

    #[test]
    fn the_details_name_the_sounds() {
        let mut i = details("h264", &["AAC", "ac3"]);
        i.audio_languages = vec!["eng".into(), "fre".into()];
        let t = tracks(Some(&i));
        assert_eq!(t.len(), 2);
        assert_eq!(
            (t[1].codec.as_str(), t[1].lang.as_deref()),
            ("ac3", Some("fre"))
        );
        // Lists of different lengths do not say which language is which.
        i.audio_languages.pop();
        assert_eq!(tracks(Some(&i))[0].lang, None);
        assert!(plays_sound("aac", Can::default()));
        assert!(!plays_sound("ac3", Can::default()));
        assert!(plays_sound(
            "ac3",
            Can {
                ac3: true,
                ..Can::default()
            }
        ));
    }
}
