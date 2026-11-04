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
//! channel's is. An MP4 with H.264 video and AAC or MP3 sound is played as
//! it is ("direct"): the browser's own player asks for byte ranges, which
//! are passed on to the provider. Anything else, or a file whose details
//! are not known, needs the remuxer ([`super::vodmux`]), which the stream
//! here does not run yet: it answers 501.
//!
//! **Limits** are live's, counted together: a device's
//! [`watch::PER_DEVICE`] viewers and [`watch::STARTS_PER_MINUTE`] starts,
//! and the service's [`watch::MAX_VIEWERS`], channels and movies alike. A
//! viewer nobody has read from for [`IDLE`] is let go.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
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
use super::server::{Resolve, Server};
use super::sharing::{Conn, Rider};
use super::tsprobe::Can;
use super::upstream::HttpUpstream;
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
    /// Bumped by each reading; an older one stops when it sees a newer.
    turn: tokio_watch::Sender<u64>,
    /// A reading (or the first opening) has the file.
    reading: bool,
    idle_since: Instant,
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
    /// place ("Watch here anyway"). One: each has a connection of its own.
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

    /// A connection among `conns` that is one of `device`'s movie viewers.
    pub(super) fn only_mine(&self, conns: &[Conn], device: &str) -> Option<i64> {
        let map = self.map();
        let mine = |rider: i64| {
            map.values()
                .any(|v| v.rider == rider && v.ended.is_none() && v.device == device)
        };
        conns
            .iter()
            .find(|c| c.riders.len() == 1 && mine(c.riders[0].id))
            .map(|c| c.feed)
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
                None => v.idle_since.elapsed() >= IDLE,
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
                turn: tokio_watch::channel(0).0,
                reading: false,
                idle_since: Instant::now(),
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
    /// The sound to play, an index into the details' list.
    #[serde(default)]
    audio: Option<usize>,
    /// Where to start, in seconds from the start of the film: the
    /// remuxer's business (direct, the browser's player seeks itself).
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
struct VodWatchDto {
    viewer: String,
    /// Where to read it: `/api/vod/stream/{viewer}`.
    stream: String,
    /// `direct` or `remux`.
    mode: &'static str,
    /// In seconds, when the details say.
    duration: Option<i64>,
    audio: Vec<TrackDto>,
    audio_index: Option<usize>,
    /// The sound is being encoded to AAC on the way.
    converting: bool,
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

// --------------------------------------------------------------- handlers

pub fn routes() -> Router<Arc<Server>> {
    Router::new()
        .route("/api/vod/watch", post(start))
        .route("/api/vod/watch/{id}", get(status).delete(stop))
        .route("/api/vod/stream/{id}", get(stream))
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
    let audio = tracks(info.as_ref());
    let audio_index = req
        .audio
        .filter(|&a| a < audio.len())
        .or((!audio.is_empty()).then_some(0));
    let mut mode = vod_mode(container.as_deref(), info.as_ref(), &can);
    // A browser's player plays a file's first sound: another is the
    // remuxer's to pick.
    if audio_index.is_some_and(|a| a > 0) {
        mode = VodMode::Remux;
    }
    let converting = mode == VodMode::Remux
        && audio_index
            .and_then(|a| audio.get(a))
            .is_some_and(|t| !plays_sound(&t.codec, can));
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
    // Counted among the service's connections while it opens.
    s.vod.map().insert(
        id.clone(),
        MovieViewer {
            device,
            movie_id,
            rider,
            mode,
            up: None,
            turn: tokio_watch::channel(0).0,
            reading: true,
            idle_since: Instant::now(),
            ended: None,
        },
    );
    // On a task of its own, so that a browser giving up on the answer
    // leaves a viewer the reaper lets go, not one opening for ever.
    let opening = tokio::spawn({
        let s = Arc::clone(&s);
        let id = id.clone();
        async move {
            let opened = match up.open(0, None).await {
                Ok(span) if span.beyond => Err(Failure::NotVideo),
                Ok(_) => Ok(()),
                Err(f) => Err(f),
            };
            let mut map = s.vod.map();
            if let (Some(v), Ok(())) = (map.get_mut(&id).filter(|v| v.ended.is_none()), &opened) {
                v.up = Some(up);
                v.reading = false;
                v.idle_since = Instant::now();
                return Ok(());
            }
            map.remove(&id);
            s.vod.release(up);
            // `None`: taken over, or stopped, while it opened.
            Err(opened.err())
        }
    });
    match opening.await {
        Ok(Ok(())) => {}
        Ok(Err(Some(f))) => return feed_refusal(f),
        Ok(Err(None)) | Err(_) => return plain(StatusCode::GONE, "feed", "This stream has ended."),
    }

    let notice = watch::notice_for(&s, movie_feed(movie_id), rider).await;
    let dto = VodWatchDto {
        viewer: id.clone(),
        stream: format!("/api/vod/stream/{id}"),
        mode: match mode {
            VodMode::Direct => "direct",
            VodMode::Remux => "remux",
        },
        duration: info.as_ref().and_then(|i| i.runtime).or(length),
        audio,
        audio_index,
        converting,
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
    let device = device_of(&caller);
    let found = s
        .vod
        .map()
        .get(&id)
        .filter(|v| v.device == device && !matches!(v.ended, Some((Ended::Stopped, _))))
        .map(|v| (v.movie_id, v.rider, v.ended));
    let Some((movie_id, rider, ended)) = found else {
        return plain(StatusCode::NOT_FOUND, "movie", "No such viewer.");
    };
    let dropped = EndingDto {
        kind: "dropped",
        status: None,
        limit: false,
    };
    let dto = match ended {
        Some((Ended::Feed(f), _)) => StatusDto {
            ended: Some(ending_of(f)),
            taken_over: false,
            notice: None,
        },
        Some((_, _)) => StatusDto {
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

async fn stream(
    State(s): State<Arc<Server>>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    req: Request,
) -> Response {
    if watch::from_another_site(req.headers()) {
        return plain(
            StatusCode::FORBIDDEN,
            "movie",
            "refused: the request came from another site",
        );
    }
    let device = device_of(&caller);
    let range = asked_range(req.headers());

    // Take the file: from the viewer, or from an older reading, which is
    // told to stop and gives it back, its response dropped.
    let deadline = Instant::now() + Duration::from_secs(3);
    let (mut up, turn, mine) = loop {
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
            if v.mode == VodMode::Remux {
                return plain(
                    StatusCode::NOT_IMPLEMENTED,
                    "notPlayable",
                    "This movie needs converting for the browser, which the service does not do yet.",
                );
            }
            let next = v.turn.borrow().wrapping_add(1);
            v.turn.send_replace(next);
            if let Some(up) = v.up.take() {
                v.reading = true;
                break (up, v.turn.subscribe(), next);
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

    // The provider asked, then the reading, on a task of their own: a
    // browser that gives up on this request while the provider answers
    // must not take the viewer's file with it.
    let (first, last) = range.unwrap_or((0, None));
    let (tx, rx) = mpsc::channel::<Bytes>(16);
    let (answer, answered) = oneshot::channel();
    let server = Arc::clone(&s);
    let reader_id = id.clone();
    tokio::spawn(async move {
        match up.open(first, last).await {
            Ok(span) if !span.beyond => {
                let _ = answer.send(Ok(span));
                let up = pass_on(up, &tx, turn, mine).await;
                give_back(&server, &reader_id, up);
            }
            Ok(span) => {
                give_back(&server, &reader_id, up);
                let _ = answer.send(Ok(span));
            }
            Err(f) => {
                // The provider will not give the file any more: the viewer
                // has ended, and its page is told why.
                let mut map = server.vod.map();
                match map.get_mut(&reader_id) {
                    Some(v) => {
                        v.reading = false;
                        v.up = Some(up);
                        server.vod.finish(v, Ended::Feed(f));
                    }
                    None => server.vod.release(up),
                }
                drop(map);
                server.vod.returned.notify_waiters();
                let _ = answer.send(Err(f));
            }
        }
    });
    let span = match answered.await {
        Ok(Ok(span)) => span,
        Ok(Err(f)) => return feed_refusal(f),
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
    if let Some(n) = span.length.filter(|&n| partial && n > 0) {
        let total = span
            .total
            .map_or_else(|| "*".to_string(), |t| t.to_string());
        let v = format!("bytes {}-{}/{total}", span.first, span.first + n - 1);
        if let Ok(v) = v.parse() {
            headers.insert(header::CONTENT_RANGE, v);
        }
    }
    let status = if partial {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    (status, headers, Body::from_stream(Chunks(rx))).into_response()
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
