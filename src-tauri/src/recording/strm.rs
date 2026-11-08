//! A recording Jellyfin can play while it records (docs/recording.md,
//! "Watching a recording in Jellyfin while it records").
//!
//! Jellyfin plays a file in its library only as far as the file went when
//! it looked, so a growing `(recording).ts` stops short. In the *Jellyfin
//! (.strm)* choice of "Show recordings to other programs while they
//! record", the part being written stays hidden and the service keeps
//! `[RECORDING] <name>.strm` beside where the `.mkv` will be instead: one
//! line, the address of the recording's live stream here,
//! `GET /api/recordings/{id}/live?key=…`. That sends the recording from its
//! first byte and then follows the part file as it grows, like `tail -f`,
//! until the recording stops capturing; then the answer ends.
//!
//! **The key** is the recording's own: 256 random bits, made when its
//! `.strm` is first written, kept only as a SHA-256 (`store::LiveKey`), and
//! good for that recording's live stream alone, while it records and for
//! [`GRACE_SECS`] after it finishes. It stands in for signing in on that
//! one route only: whoever can read the recording's folder can watch that
//! recording while it records, and nothing else. A key is compared as its
//! hash, the same length whatever was sent, without stopping at the first
//! difference; wrong ones are counted per address and slowed down like
//! wrong passwords (`access::Limiter`), apart from them. A key is never
//! logged. The `.strm` keeps its key across restarts of the service (the
//! one in the file is kept when it matches the hash on record), so what
//! Jellyfin read from it stays good.
//!
//! **Parts.** A recording that switched feeds has several parts; the stream
//! carries on from the end of one into the next, with the new feed's
//! packets as they came (a player sees a jump in time, and a new set of
//! streams if the feed's differ). Only parts still being captured, or not
//! yet finished into their `.mkv`, are in it: what a recording interrupted
//! by a restart of the service already finished into `… - part1.mkv` is
//! not.
//!
//! **Ranges.** `Range: bytes=N-` starts the stream at byte N of it (a
//! player resuming), as far as what is on disk goes; `bytes=N-M` is that
//! slice as it is now. Nothing past what is written can be asked for.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use bytes::Bytes;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::mpsc;

use super::access::{self, Peer};
use super::api::{ErrorDto, PartDto, RecordingState, WhileRecording};
use super::config::{token_matches, NetworkConfig};
use super::server::Server;
use super::store::{self, LiveKey};
use super::watch::Chunks;
use super::{naming, now};

/// Random bytes in a key.
const KEY_BYTES: usize = 32;
/// How long after its recording finished a key still opens its stream:
/// enough for a player reading the end, or reconnecting as it finishes.
pub const GRACE_SECS: i64 = 60;
/// How often the end of a growing part is looked at again.
const POLL: Duration = Duration::from_millis(250);
const CHUNK: usize = 64 * 1024;
/// Live streams read at once, from every recording together.
pub const MAX_READERS: usize = 32;

// ------------------------------------------------------------ the address

/// Where `.strm` files point: `scheme://host:port`.
///
/// The host is the one chosen in Settings, or else, with network access
/// on, this computer's likeliest address on the network (`ips`, as
/// `listen::interface_ips` ranks them), and with it off 127.0.0.1, which
/// only a Jellyfin on this computer reaches. HTTPS whenever network access
/// wants it, except to this computer, which is always served plain HTTP.
pub fn base_url(host: &str, net: NetworkConfig, port: u16, ips: &[IpAddr]) -> String {
    let host = host.trim();
    let chosen = if !host.is_empty() {
        host.to_string()
    } else if net.enabled {
        ips.first()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "127.0.0.1".into())
    } else {
        "127.0.0.1".into()
    };
    let bare = chosen
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(&chosen);
    let local = bare.eq_ignore_ascii_case("localhost")
        || bare.parse::<IpAddr>().is_ok_and(access::is_loopback);
    let scheme = if !local && net.enabled && net.https {
        "https"
    } else {
        "http"
    };
    if bare.contains(':') {
        format!("{scheme}://[{bare}]:{port}")
    } else {
        format!("{scheme}://{bare}:{port}")
    }
}

/// The live stream of recording `id`, as a `.strm` file names it.
pub fn url(base: &str, id: i64, key: &str) -> String {
    format!("{base}/api/recordings/{id}/live?key={key}")
}

/// The key in a `.strm` file's text, when the file is one of ours for
/// recording `id`.
fn key_in(text: &str, id: i64) -> Option<&str> {
    let line = text.lines().next()?.trim();
    let (_, key) = line.split_once(&format!("/api/recordings/{id}/live?key="))?;
    (!key.is_empty() && key.bytes().all(|b| b.is_ascii_hexdigit())).then_some(key)
}

// ---------------------------------------------------------------- the key

fn new_key() -> Result<String> {
    access::random_hex(KEY_BYTES)
}

fn hash(key: &str) -> String {
    access::token_hash(key)
}

/// What a key given for a recording's live stream is worth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCheck {
    Valid,
    /// None was given.
    Missing,
    /// The recording's own, but it has finished (and the grace is over).
    Expired,
    /// Not the recording's: a guess, or another recording's key.
    Wrong,
}

/// Checks `given` against the key `kept` for a recording now in `state`.
pub fn check(
    kept: Option<&LiveKey>,
    state: Option<RecordingState>,
    given: Option<&str>,
    now: i64,
) -> KeyCheck {
    let Some(given) = given.filter(|g| !g.is_empty()) else {
        return KeyCheck::Missing;
    };
    // A recording with no key, or none at all, is answered as a wrong key
    // is, so the answer says nothing of which recordings exist.
    let Some(kept) = kept else {
        return KeyCheck::Wrong;
    };
    // Hash against hash: the same length whatever was sent, compared
    // without stopping at the first difference.
    if !token_matches(&kept.hash, &hash(given)) {
        return KeyCheck::Wrong;
    }
    match state {
        Some(RecordingState::Recording | RecordingState::Finishing) => KeyCheck::Valid,
        _ if kept.finished.is_some_and(|f| now < f + GRACE_SECS) => KeyCheck::Valid,
        _ => KeyCheck::Expired,
    }
}

// -------------------------------------------------------------- the files

/// Writes recording `id`'s `.strm`, or brings it up to date with how the
/// service is reached now. The key in the file already there is kept when
/// it is the one on record, so the file — and what a media server read from
/// it — stays the same from one start of the service to the next; otherwise
/// a new key is made.
pub fn ensure(server: &Server, id: i64) -> Result<()> {
    // One at a time: a capture starting while a change of settings writes
    // every .strm must not leave the file with one key and the record with
    // another.
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let Some(rec) = server.db(|c| store::get(c, id))? else {
        return Ok(());
    };
    let info = server.db(|c| store::capture_info(c, id))?;
    let kept = server.db(|c| store::live_key(c, id))?;
    let folder = PathBuf::from(&rec.folder);
    let path = match kept.as_ref().and_then(|k| k.strm.clone()) {
        Some(p) => PathBuf::from(p),
        None => {
            std::fs::create_dir_all(&folder)
                .with_context(|| format!("creating {}", folder.display()))?;
            // Two recordings of one airing (on two channels) share a name.
            let name = naming::unique(&naming::strm_file(&info.base), |n| {
                folder.join(n).exists()
            });
            folder.join(name)
        }
    };
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let key = match (key_in(&old, id), &kept) {
        (Some(k), Some(kept)) if kept.strm.is_some() && token_matches(&kept.hash, &hash(k)) => {
            k.to_string()
        }
        _ => new_key()?,
    };
    let path_text = path.to_string_lossy().into_owned();
    let hashed = hash(&key);
    if kept
        .as_ref()
        .is_none_or(|k| k.hash != hashed || k.strm.as_deref() != Some(&path_text))
    {
        server.db(|c| store::set_live_key(c, id, &hashed, &path_text))?;
    }
    let text = format!("{}\n", url(&server.strm_base(), id, &key));
    if old != text {
        write_whole(&path, &text)?;
    }
    Ok(())
}

/// Writes `text` to `path` through a hidden file beside it, so a library
/// scan never reads half of it.
fn write_whole(path: &Path, text: &str) -> Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn remove(path: &str) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("could not remove {path}: {e}"),
    }
}

/// Recording `id` has finished: its `.strm` goes, and its key lapses
/// [`GRACE_SECS`] from now.
pub fn finished(server: &Server, id: i64) {
    let Ok(Some(k)) = server.db(|c| store::live_key(c, id)) else {
        return;
    };
    if let Some(p) = &k.strm {
        remove(p);
    }
    if k.finished.is_none() {
        if let Err(e) = server.db(|c| store::finish_live_key(c, id, now())) {
            tracing::warn!("recording {id}: could not retire its live stream key: {e:#}");
        }
    }
}

/// Recording `id` is forgotten: its `.strm` goes with it, if one is left.
pub fn forget(server: &Server, id: i64) {
    if let Ok(Some(LiveKey { strm: Some(p), .. })) = server.db(|c| store::live_key(c, id)) {
        remove(&p);
    }
}

/// Writes or updates the `.strm` of every recording capturing now, when
/// that is the choice: after a change of settings or of network access,
/// which may change the address in them.
pub fn refresh(server: &Server) {
    if server.config().while_recording != WhileRecording::Strm {
        return;
    }
    let ids: Vec<i64> = server.live(|m| {
        m.iter()
            .filter(|(_, l)| !l.finishing)
            .map(|(&id, _)| id)
            .collect()
    });
    for id in ids {
        if let Err(e) = ensure(server, id) {
            server.note(id, &format!("could not write its .strm file: {e:#}"));
        }
    }
}

/// When the service starts: the `.strm` files of recordings that are over
/// (finished, failed, or forgotten while the service was not running to
/// clean up) go, and those of recordings that carry on are written again,
/// for how the service is reached now. One whose window has passed is left
/// for finishing it to remove.
pub fn recover(server: &Server) -> Result<()> {
    let t = now();
    let strm = server.config().while_recording == WhileRecording::Strm;
    for (id, path) in server.db(store::with_strm)? {
        let rec = server.db(|c| store::get(c, id))?;
        match rec {
            Some(r) if matches!(r.state, RecordingState::Recording | RecordingState::Finishing) => {
                if strm && r.state == RecordingState::Recording && t < r.stop + r.pad_after {
                    if let Err(e) = ensure(server, id) {
                        server.note(id, &format!("could not write its .strm file: {e:#}"));
                    }
                }
            }
            r => {
                remove(&path);
                let ended = r.map_or(t, |r| r.stop + r.pad_after).min(t);
                server.db(|c| store::finish_live_key(c, id, ended))?;
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------ the stream

pub fn routes() -> Router<Arc<Server>> {
    Router::new().route("/api/recordings/{id}/live", get(live))
}

#[derive(Debug, Deserialize)]
struct LiveQuery {
    key: Option<String>,
}

fn error(status: StatusCode, text: &str) -> Response {
    (
        status,
        Json(ErrorDto {
            error: text.into(),
        }),
    )
        .into_response()
}

fn too_many(wait: Duration) -> Response {
    let secs = wait.as_secs_f64().ceil().max(1.0) as u64;
    let mut r = error(
        StatusCode::TOO_MANY_REQUESTS,
        &format!("too many wrong keys; try again in {secs} s"),
    );
    if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
        r.headers_mut().insert(header::RETRY_AFTER, v);
    }
    r
}

/// `Range: bytes=N-` or `bytes=N-M`: one range from a known start. Any
/// other (a suffix, several) is not one this stream answers, and is read as
/// no range at all, as HTTP allows.
fn parse_range(v: &str) -> Option<(u64, Option<u64>)> {
    let spec = v.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (from, to) = spec.split_once('-')?;
    let from: u64 = from.trim().parse().ok()?;
    let to = match to.trim() {
        "" => None,
        t => Some(t.parse::<u64>().ok().filter(|&t| t >= from)?),
    };
    Some((from, to))
}

/// The parts of a recording the stream is made of: its capture files, in
/// order. Not a part handed over by the app (what its player had buffered of
/// the programme): it arrives after the stream has begun and goes in front
/// of what was received, so following it would move every byte a reader
/// has already been sent (a `Range` resuming would land elsewhere, and one
/// reading on would be sent the start after the middle). It is in the
/// finished files.
fn followable(parts: &[PartDto], base: &str) -> Vec<PartDto> {
    parts
        .iter()
        .filter(|p| !p.handed_over)
        .filter(|p| {
            Path::new(&p.path)
                .file_name()
                .is_some_and(|n| naming::is_capture_file(&n.to_string_lossy(), base))
        })
        .cloned()
        .collect()
}

/// Whether recording `id` is capturing: more may come.
fn capturing(server: &Server, id: i64) -> bool {
    server.live(|m| m.get(&id).is_some_and(|l| !l.finishing))
}

/// The recording's parts as they are now, and whether it captures still.
struct Look {
    parts: Vec<PartDto>,
    capturing: bool,
}

fn look(server: &Server, id: i64, base: &str) -> Option<Look> {
    // Whether it captures is asked first: a part it closes after that is
    // then on record by the time the parts are read.
    let capturing = capturing(server, id);
    let rec = server.db(|c| store::get(c, id)).ok().flatten()?;
    Some(Look {
        parts: followable(&rec.parts, base),
        capturing,
    })
}

/// The readers of live streams now, given back as each one ends.
static READERS: AtomicUsize = AtomicUsize::new(0);

struct Reader;

impl Reader {
    fn take() -> Option<Self> {
        // A compare-and-swap loop rather than `fetch_update`, which newer Rust
        // deprecates under a new name the older toolchains don't have.
        let mut n = READERS.load(Ordering::SeqCst);
        loop {
            if n >= MAX_READERS {
                return None;
            }
            match READERS.compare_exchange_weak(n, n + 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return Some(Reader),
                Err(now) => n = now,
            }
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        READERS.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn live(
    State(s): State<Arc<Server>>,
    UrlPath(id): UrlPath<i64>,
    Query(q): Query<LiveQuery>,
    p: Option<Extension<Peer>>,
    headers: HeaderMap,
) -> Response {
    // Always there through the listener; without one, the address the
    // limiter counts is nobody's in particular.
    let ip = p.map_or(IpAddr::from([0, 0, 0, 0]), |Extension(p)| p.addr.ip());
    if let Err(wait) = s.access.key_allowed(ip) {
        return too_many(wait);
    }
    let kept = s.db(|c| store::live_key(c, id)).ok().flatten();
    let rec = s.db(|c| store::get(c, id)).ok().flatten();
    match check(kept.as_ref(), rec.as_ref().map(|r| r.state), q.key.as_deref(), now()) {
        KeyCheck::Valid => s.access.key_right(ip),
        KeyCheck::Missing => {
            return error(
                StatusCode::UNAUTHORIZED,
                "this stream needs the key its .strm file gives",
            )
        }
        KeyCheck::Expired => {
            return error(
                StatusCode::GONE,
                "this recording has finished: play its file instead",
            )
        }
        KeyCheck::Wrong => {
            tracing::warn!("a live stream was asked for from {ip} with a wrong key");
            return match s.access.key_wrong(ip) {
                Some(wait) => too_many(wait),
                None => error(StatusCode::UNAUTHORIZED, "wrong key"),
            };
        }
    }
    let Some(rec) = rec else {
        return error(StatusCode::GONE, "this recording is gone");
    };
    let Ok(info) = s.db(|c| store::capture_info(c, id)) else {
        return error(StatusCode::GONE, "this recording is gone");
    };
    let parts = followable(&rec.parts, &info.base);
    if parts.is_empty() && !capturing(&s, id) {
        return error(
            StatusCode::GONE,
            "this recording has finished: play its file instead",
        );
    }
    let written: u64 = parts
        .iter()
        .map(|p| std::fs::metadata(&p.path).map_or(0, |m| m.len()))
        .sum();

    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_range);
    let (from, limit, content_range) = match range {
        None => (0, None, None),
        Some((from, _)) if from >= written => {
            let mut r = error(
                StatusCode::RANGE_NOT_SATISFIABLE,
                "that is past what has been recorded so far",
            );
            if let Ok(v) = HeaderValue::from_str(&format!("bytes */{written}")) {
                r.headers_mut().insert(header::CONTENT_RANGE, v);
            }
            return r;
        }
        // A slice: as it is on disk now, whole.
        Some((from, Some(to))) => {
            let to = to.min(written - 1);
            (from, Some(to - from + 1), Some(format!("bytes {from}-{to}/*")))
        }
        // From there on: what is on disk now, and then on as it grows. The
        // range names what was there when it answered.
        Some((from, None)) => (from, None, Some(format!("bytes {from}-{}/*", written - 1))),
    };

    let Some(reader) = Reader::take() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "too many live streams are being read; try again later",
        );
    };
    let (tx, rx) = mpsc::channel::<Bytes>(16);
    let server = Arc::clone(&s);
    tokio::spawn(async move {
        let _reader = reader;
        follow(&server, id, &info.base, from, limit, &tx).await;
    });

    let mut r = Body::from_stream(Chunks(rx)).into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp2t"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // A media server's own web page may play it straight from here (its
    // origin is not this one); the key, not the origin, is what guards it.
    h.insert(
        header::HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("cross-origin"),
    );
    if let Some(cr) = content_range.and_then(|c| HeaderValue::from_str(&c).ok()) {
        *r.status_mut() = StatusCode::PARTIAL_CONTENT;
        r.headers_mut().insert(header::CONTENT_RANGE, cr);
        if let Some(n) = limit {
            r.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from(n));
        }
    }
    r
}

/// Sends recording `id` (named `base`) from byte `skip` of its stream, as
/// it is and as it grows, until it stops capturing, `limit` bytes are sent,
/// or the reader leaves.
async fn follow(
    server: &Server,
    id: i64,
    base: &str,
    mut skip: u64,
    mut limit: Option<u64>,
    tx: &mpsc::Sender<Bytes>,
) {
    // The parts read to their end, and the one being read.
    let mut done: Vec<String> = Vec::new();
    let mut current: Option<(String, tokio::fs::File)> = None;
    let mut buf = vec![0u8; CHUNK];
    loop {
        if limit == Some(0) || tx.is_closed() {
            return;
        }
        let Some((path, file)) = current.as_mut() else {
            let Some(now) = look(server, id, base) else {
                return;
            };
            let Some(next) = now.parts.iter().find(|p| !done.contains(&p.path)) else {
                if !now.capturing {
                    return;
                }
                // Waiting for a connection, or for the next part.
                pause(tx).await;
                continue;
            };
            // A part that cannot be opened has been finished into its file
            // since: the stream is over.
            let Ok(mut f) = tokio::fs::File::open(&next.path).await else {
                return;
            };
            if skip > 0 {
                let len = f.metadata().await.map_or(0, |m| m.len());
                let ended = next.ended.is_some();
                if skip >= len && ended {
                    skip -= len;
                    done.push(next.path.clone());
                    continue;
                }
                let at = skip.min(len);
                if f.seek(std::io::SeekFrom::Start(at)).await.is_err() {
                    return;
                }
                skip -= at;
            }
            current = Some((next.path.clone(), f));
            continue;
        };
        let want = limit.map_or(CHUNK, |l| l.min(CHUNK as u64) as usize);
        let n = match file.read(&mut buf[..want]).await {
            Ok(0) => {
                // At the end of what is written. Whether the part can grow
                // is looked at before reading once more, so an empty read
                // after a "no" is the part's real end: a capture closes a
                // part, all of it written, before it says so.
                let Some(now) = look(server, id, base) else {
                    return;
                };
                let at = now.parts.iter().position(|p| &p.path == path);
                let over = !now.capturing
                    || at.is_none_or(|i| now.parts[i].ended.is_some() || i + 1 < now.parts.len());
                match file.read(&mut buf[..want]).await {
                    Ok(0) if over => {
                        done.push(path.clone());
                        current = None;
                        continue;
                    }
                    Ok(0) => {
                        pause(tx).await;
                        continue;
                    }
                    Ok(n) => n,
                    Err(_) => return,
                }
            }
            Ok(n) => n,
            Err(_) => return,
        };
        if tx.send(Bytes::copy_from_slice(&buf[..n])).await.is_err() {
            return;
        }
        if let Some(l) = limit.as_mut() {
            *l -= n as u64;
        }
    }
}

/// A moment, or less if the reader leaves.
async fn pause(tx: &mpsc::Sender<Bytes>) {
    tokio::select! {
        _ = tokio::time::sleep(POLL) => {}
        _ = tx.closed() => {}
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::pin::Pin;

    use axum::extract::Request;
    use tower::ServiceExt;

    use super::*;
    use crate::recording::api::ScheduleRequest;
    use crate::recording::http::{router, tests::server};
    use crate::recording::server::Live;

    fn kept(key: &str, finished: Option<i64>) -> LiveKey {
        LiveKey {
            hash: hash(key),
            strm: Some("/v/x.strm".into()),
            finished,
        }
    }

    #[test]
    fn a_key_opens_its_own_recording_while_it_records_and_a_moment_after() {
        let key = new_key().unwrap();
        assert_eq!(key.len(), KEY_BYTES * 2, "256 bits, in hex");
        assert_ne!(key, new_key().unwrap());
        let k = kept(&key, None);
        let rec = Some(RecordingState::Recording);
        assert_eq!(check(Some(&k), rec, Some(&key), 0), KeyCheck::Valid);
        assert_eq!(
            check(Some(&k), Some(RecordingState::Finishing), Some(&key), 0),
            KeyCheck::Valid
        );
        // Wrong, cut short, made longer, or another recording's.
        let other = new_key().unwrap();
        for bad in [other.as_str(), &key[..key.len() - 1], &format!("{key}0"), "0"] {
            assert_eq!(check(Some(&k), rec, Some(bad), 0), KeyCheck::Wrong, "{bad}");
        }
        assert_eq!(check(Some(&k), rec, None, 0), KeyCheck::Missing);
        assert_eq!(check(Some(&k), rec, Some(""), 0), KeyCheck::Missing);
        // A recording with no key (or none at all) says the same as a wrong
        // key would.
        assert_eq!(check(None, rec, Some(&key), 0), KeyCheck::Wrong);
        assert_eq!(check(None, None, Some(&key), 0), KeyCheck::Wrong);
        // Finished: good for the grace, then over.
        let k = kept(&key, Some(1000));
        let done = Some(RecordingState::Done);
        assert_eq!(check(Some(&k), done, Some(&key), 1000), KeyCheck::Valid);
        assert_eq!(
            check(Some(&k), done, Some(&key), 1000 + GRACE_SECS - 1),
            KeyCheck::Valid
        );
        assert_eq!(
            check(Some(&k), done, Some(&key), 1000 + GRACE_SECS),
            KeyCheck::Expired
        );
        assert_eq!(
            check(Some(&k), Some(RecordingState::Failed), Some(&other), 1000),
            KeyCheck::Wrong
        );
        // Kept as its hash alone.
        assert!(!k.hash.contains(&key) && k.hash.len() == 64);
    }

    #[test]
    fn keys_are_compared_as_hashes_of_one_length() {
        // Whatever is sent, what is compared is a SHA-256 in hex: a guess's
        // length says nothing, and `token_matches` goes through every byte.
        for given in ["", "a", &"f".repeat(64), &"f".repeat(500)] {
            assert_eq!(hash(given).len(), 64);
        }
        let key = new_key().unwrap();
        assert!(token_matches(&hash(&key), &hash(&key)));
        assert!(!token_matches(&hash(&key), &hash(&key.to_uppercase())));
    }

    #[test]
    fn the_address_is_the_one_other_devices_reach() {
        let lan: Vec<IpAddr> = vec!["192.168.1.20".parse().unwrap(), "fd00::20".parse().unwrap()];
        let on = |https| NetworkConfig {
            enabled: true,
            https,
        };
        let off = NetworkConfig {
            enabled: false,
            https: true,
        };
        assert_eq!(base_url("", on(true), 8796, &lan), "https://192.168.1.20:8796");
        assert_eq!(base_url("", on(false), 8796, &lan), "http://192.168.1.20:8796");
        assert_eq!(base_url("", on(true), 8796, &[]), "http://127.0.0.1:8796");
        // Network access off: only this computer reaches it.
        assert_eq!(base_url("", off, 9000, &lan), "http://127.0.0.1:9000");
        // A host chosen by hand; this computer is always plain HTTP.
        assert_eq!(base_url("nas.local", on(true), 8796, &lan), "https://nas.local:8796");
        assert_eq!(base_url("127.0.0.1", on(true), 8796, &lan), "http://127.0.0.1:8796");
        assert_eq!(base_url("localhost", on(true), 8796, &lan), "http://localhost:8796");
        assert_eq!(base_url("fd00::20", on(true), 8796, &lan), "https://[fd00::20]:8796");
        assert_eq!(base_url("[::1]", on(true), 8796, &lan), "http://[::1]:8796");
        assert_eq!(
            url("http://127.0.0.1:8796", 7, "abc"),
            "http://127.0.0.1:8796/api/recordings/7/live?key=abc"
        );
        let text = format!("{}\n", url("https://h:1", 7, "00ff"));
        assert_eq!(key_in(&text, 7), Some("00ff"));
        assert_eq!(key_in(&text, 8), None, "another recording's");
        assert_eq!(key_in("http://h:1/api/recordings/7/live?key=zz", 7), None);
        assert_eq!(key_in("", 7), None);
    }

    #[test]
    fn ranges_from_a_known_start_are_read_and_others_ignored() {
        assert_eq!(parse_range("bytes=0-"), Some((0, None)));
        assert_eq!(parse_range("bytes=1000-"), Some((1000, None)));
        assert_eq!(parse_range("bytes=10-19"), Some((10, Some(19))));
        for other in ["bytes=-500", "bytes=0-1,5-6", "items=0-", "bytes=9-3", "bytes=x-"] {
            assert_eq!(parse_range(other), None, "{other}");
        }
    }

    // ---------------------------------------------- against the service

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zapper-strm-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A recording on record, in `state`, in `folder`; and, while
    /// `capturing`, a capture of it as the service would hold one.
    fn recording(s: &Server, folder: &Path, base: &str, state: RecordingState) -> i64 {
        let t = now();
        s.db(|c| {
            let req = ScheduleRequest {
                stream_id: 7,
                backup_stream_id: None,
                channel_name: Some("One".into()),
                title: base.into(),
                episode_title: None,
                season: None,
                episode: None,
                start: t - 60,
                stop: t + 3600,
                pad_before: None,
                pad_after: None,
            };
            let id = store::insert(
                c,
                &store::NewRecording {
                    req: &req,
                    backup_stream_id: None,
                    channel_name: "One",
                    pad_before: 0,
                    pad_after: 0,
                    folder: &folder.to_string_lossy(),
                    base,
                    clock_track: false,
                    now: t,
                },
            )?;
            store::set_state(c, id, state, None)?;
            Ok(id)
        })
        .unwrap()
    }

    fn capture(s: &Server, id: i64) {
        let (tx, _) = tokio::sync::watch::channel(false);
        s.live(|m| {
            m.insert(
                id,
                Live {
                    title: "x".into(),
                    stream_id: 7,
                    bytes: 0,
                    since: now(),
                    waiting: None,
                    retry_at: None,
                    until: now() + 3600,
                    finishing: false,
                    stop: tx,
                },
            )
        });
    }

    fn stop_capturing(s: &Server, id: i64) {
        s.update_live(id, |l| l.finishing = true);
        s.db(|c| store::mark(c, id, RecordingState::Finishing)).unwrap();
    }

    fn part(path: &Path, ended: bool) -> PartDto {
        PartDto {
            path: path.to_string_lossy().into_owned(),
            stream_id: 7,
            bytes: 0,
            started: now(),
            ended: ended.then(now),
            handed_over: false,
        }
    }

    fn strm_mode(s: &Server) {
        let mut c = s.config();
        c.while_recording = WhileRecording::Strm;
        s.replace_config(c);
    }

    /// The key in recording `id`'s `.strm`, and the file.
    fn strm_of(s: &Server, id: i64) -> (String, PathBuf) {
        let k = s.db(|c| store::live_key(c, id)).unwrap().unwrap();
        let path = PathBuf::from(k.strm.unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        (key_in(&text, id).unwrap().to_string(), path)
    }

    fn get(uri: &str, from: &str, range: Option<&str>) -> Request {
        let mut b = Request::builder().uri(uri);
        if let Some(r) = range {
            b = b.header(header::RANGE, r);
        }
        let mut r = b.body(Body::empty()).unwrap();
        r.extensions_mut().insert(Peer {
            addr: SocketAddr::new(from.parse().unwrap(), 40000),
            tls: false,
        });
        r
    }

    async fn call(s: &Arc<Server>, r: Request) -> Response {
        router(s.clone()).oneshot(r).await.unwrap()
    }

    /// The next chunk of a body, if one comes within a few seconds.
    async fn next(body: &mut axum::body::BodyDataStream) -> Option<Bytes> {
        use futures_core::Stream;
        let f = std::future::poll_fn(|cx| Pin::new(&mut *body).poll_next(cx));
        tokio::time::timeout(Duration::from_secs(5), f)
            .await
            .expect("the stream went quiet")
            .map(|r| r.unwrap())
    }

    /// Reads until `want` bytes have come.
    async fn read(body: &mut axum::body::BodyDataStream, want: usize) -> Vec<u8> {
        let mut got = Vec::new();
        while got.len() < want {
            got.extend_from_slice(&next(body).await.expect("the stream ended early"));
        }
        got
    }

    fn append(path: &Path, data: &[u8]) {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(data).unwrap();
    }

    fn set_parts(s: &Server, id: i64, parts: &[PartDto]) {
        s.db(|c| store::set_parts(c, id, parts)).unwrap();
    }

    #[tokio::test]
    async fn the_stream_follows_a_growing_part_and_ends_with_the_recording() {
        let s = server("strm-follow");
        strm_mode(&s);
        let dir = scratch("follow");
        let base = "Harbour Lights - 2026-11-03 20.00";
        let id = recording(&s, &dir, base, RecordingState::Recording);
        capture(&s, id);
        let ts = dir.join(naming::capture_file(base, 1, false));
        append(&ts, &[1u8; 1000]);
        set_parts(&s, id, &[part(&ts, false)]);
        ensure(&s, id).unwrap();
        let (key, strm) = strm_of(&s, id);
        assert_eq!(
            strm.file_name().unwrap().to_string_lossy(),
            format!("[RECORDING] {base}.strm")
        );
        let uri = format!("/api/recordings/{id}/live?key={key}");

        let resp = call(&s, get(&uri, "127.0.0.1", None)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()[header::CONTENT_TYPE], "video/mp2t");
        assert!(resp.headers().get(header::CONTENT_LENGTH).is_none());
        let mut body = resp.into_body().into_data_stream();
        assert_eq!(read(&mut body, 1000).await, vec![1u8; 1000]);
        // Written after it connected, in chunks: it comes too.
        for b in 2..=4u8 {
            append(&ts, &[b; 300]);
            assert_eq!(read(&mut body, 300).await, vec![b; 300]);
        }
        // The capture closes the part with its last bytes and stops: the
        // stream sends them and ends.
        append(&ts, &[9u8; 50]);
        set_parts(&s, id, &[part(&ts, true)]);
        stop_capturing(&s, id);
        assert_eq!(read(&mut body, 50).await, vec![9u8; 50]);
        assert_eq!(next(&mut body).await, None, "it ends with the recording");

        // Finished: the .strm goes, and the key is good for a moment more,
        // then not.
        s.db(|c| store::set_state(c, id, RecordingState::Done, None)).unwrap();
        std::fs::remove_file(&ts).unwrap();
        set_parts(&s, id, &[]);
        finished(&s, id);
        assert!(!strm.exists());
        assert_eq!(call(&s, get(&uri, "127.0.0.1", None)).await.status(), StatusCode::GONE);
        s.db(|c| store::finish_live_key(c, id, now() - GRACE_SECS)).unwrap();
        s.live(|m| m.remove(&id));
        assert_eq!(call(&s, get(&uri, "127.0.0.1", None)).await.status(), StatusCode::GONE);
        assert_eq!(
            check(
                s.db(|c| store::live_key(c, id)).unwrap().as_ref(),
                Some(RecordingState::Done),
                Some(&key),
                now()
            ),
            KeyCheck::Expired
        );
    }

    #[tokio::test]
    async fn the_stream_carries_on_into_the_next_part() {
        let s = server("strm-parts");
        strm_mode(&s);
        let dir = scratch("parts");
        let base = "News - 2026-11-03 18.00";
        let id = recording(&s, &dir, base, RecordingState::Recording);
        capture(&s, id);
        let one = dir.join(naming::capture_file(base, 1, false));
        append(&one, &[1u8; 500]);
        set_parts(&s, id, &[part(&one, false)]);
        ensure(&s, id).unwrap();
        let (key, _) = strm_of(&s, id);
        let uri = format!("/api/recordings/{id}/live?key={key}");
        let mut body = call(&s, get(&uri, "127.0.0.1", None))
            .await
            .into_body()
            .into_data_stream();
        assert_eq!(read(&mut body, 500).await, vec![1u8; 500]);

        // The feed fails over: part 1 closes, and a while later part 2
        // begins on the backup.
        append(&one, &[2u8; 100]);
        set_parts(&s, id, &[part(&one, true)]);
        assert_eq!(read(&mut body, 100).await, vec![2u8; 100]);
        tokio::time::sleep(POLL * 2).await;
        let two = dir.join(naming::capture_file(base, 2, false));
        append(&two, &[3u8; 400]);
        set_parts(&s, id, &[part(&one, true), part(&two, false)]);
        assert_eq!(read(&mut body, 400).await, vec![3u8; 400]);

        // A player resuming at byte 550 gets the rest of part 1 and on.
        let resp = call(&s, get(&uri, "127.0.0.1", Some("bytes=550-"))).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(resp.headers()[header::CONTENT_RANGE], "bytes 550-999/*");
        let mut resumed = resp.into_body().into_data_stream();
        let got = read(&mut resumed, 450).await;
        assert_eq!((&got[..50], &got[50..]), (&[2u8; 50][..], &[3u8; 400][..]));
        // A slice is what is there now, whole.
        let resp = call(&s, get(&uri, "127.0.0.1", Some("bytes=490-509"))).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(resp.headers()[header::CONTENT_LENGTH], "20");
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        assert_eq!(&bytes[..], &[[1u8; 10], [2u8; 10]].concat()[..]);
        // Nothing past what is written.
        let resp = call(&s, get(&uri, "127.0.0.1", Some("bytes=1000-"))).await;
        assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(resp.headers()[header::CONTENT_RANGE], "bytes */1000");

        append(&two, &[4u8; 10]);
        set_parts(&s, id, &[part(&one, true), part(&two, true)]);
        stop_capturing(&s, id);
        assert_eq!(read(&mut body, 10).await, vec![4u8; 10]);
        assert_eq!(next(&mut body).await, None);
        assert_eq!(read(&mut resumed, 10).await, vec![4u8; 10]);
        assert_eq!(next(&mut resumed).await, None);
    }

    #[tokio::test]
    async fn a_part_handed_over_in_front_moves_nothing_a_reader_has() {
        let s = server("strm-handed-over");
        strm_mode(&s);
        let dir = scratch("handed-over");
        let base = "News - 2026-11-03 18.00";
        let id = recording(&s, &dir, base, RecordingState::Recording);
        capture(&s, id);
        let live = dir.join(naming::capture_file(base, 1, false));
        append(&live, &[1u8; 500]);
        set_parts(&s, id, &[part(&live, false)]);
        ensure(&s, id).unwrap();
        let (key, _) = strm_of(&s, id);
        let uri = format!("/api/recordings/{id}/live?key={key}");
        let mut body = call(&s, get(&uri, "127.0.0.1", None))
            .await
            .into_body()
            .into_data_stream();
        assert_eq!(read(&mut body, 500).await, vec![1u8; 500]);

        // The app hands over what its player had buffered: a part in front
        // of the one being followed.
        let buffer = dir.join(naming::numbered(&naming::capture_file(base, 1, false), 2));
        append(&buffer, &[7u8; 300]);
        let handed = PartDto {
            handed_over: true,
            ..part(&buffer, true)
        };
        set_parts(&s, id, &[handed.clone(), part(&live, false)]);

        // The reader carries on where it was, and never gets the buffer.
        append(&live, &[2u8; 100]);
        assert_eq!(read(&mut body, 100).await, vec![2u8; 100]);
        // Resuming names the same bytes as before.
        let resp = call(&s, get(&uri, "127.0.0.1", Some("bytes=550-"))).await;
        assert_eq!(resp.headers()[header::CONTENT_RANGE], "bytes 550-599/*");
        let mut resumed = resp.into_body().into_data_stream();
        assert_eq!(read(&mut resumed, 50).await, vec![2u8; 50]);
        // The part being received closes: the stream ends there, rather
        // than going back to the buffer.
        set_parts(&s, id, &[handed, part(&live, true)]);
        stop_capturing(&s, id);
        assert_eq!(next(&mut body).await, None);
        assert_eq!(next(&mut resumed).await, None);
    }

    #[tokio::test]
    async fn only_the_recordings_own_key_opens_it_and_guesses_are_slowed() {
        let s = server("strm-keys");
        strm_mode(&s);
        let dir = scratch("keys");
        let a = recording(&s, &dir, "A - 2026-11-03 20.00", RecordingState::Recording);
        let b = recording(&s, &dir, "B - 2026-11-03 20.00", RecordingState::Recording);
        capture(&s, a);
        capture(&s, b);
        for id in [a, b] {
            let base = s.db(|c| store::capture_info(c, id)).unwrap().base;
            let ts = dir.join(naming::capture_file(&base, 1, false));
            append(&ts, &[1u8; 10]);
            set_parts(&s, id, &[part(&ts, false)]);
            ensure(&s, id).unwrap();
        }
        let (ka, _) = strm_of(&s, a);
        let (kb, _) = strm_of(&s, b);
        assert_ne!(ka, kb);
        let status = |uri: String, from: &'static str| {
            let s = s.clone();
            async move { call(&s, get(&uri, from, None)).await.status() }
        };
        assert_eq!(status(format!("/api/recordings/{a}/live?key={ka}"), "127.0.0.1").await, StatusCode::OK);
        // B's key is not A's; no key is not a key; the app's sign-in does
        // not stand in for it either.
        assert_eq!(
            status(format!("/api/recordings/{a}/live?key={kb}"), "127.0.0.1").await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(status(format!("/api/recordings/{a}/live"), "127.0.0.1").await, StatusCode::UNAUTHORIZED);
        assert_eq!(
            status(format!("/api/recordings/999/live?key={ka}"), "127.0.0.1").await,
            StatusCode::UNAUTHORIZED,
            "no such recording reads as a wrong key"
        );
        // It opens that one stream and nothing else.
        for other in [format!("/api/recordings/{a}?key={ka}"), format!("/api/status?key={ka}")] {
            assert_eq!(status(other, "127.0.0.1").await, StatusCode::UNAUTHORIZED);
        }
        let mut bearer = get("/api/recordings", "127.0.0.1", None);
        bearer
            .headers_mut()
            .insert(header::AUTHORIZATION, format!("Bearer {ka}").parse().unwrap());
        assert_eq!(call(&s, bearer).await.status(), StatusCode::UNAUTHORIZED);
        // From another machine, only with network access on.
        assert_eq!(
            status(format!("/api/recordings/{a}/live?key={ka}"), "192.168.1.30").await,
            StatusCode::FORBIDDEN
        );
        // Wrong keys from one address: a few, then a wait — for that
        // address alone, and apart from its sign-ins.
        let mut last = StatusCode::OK;
        for _ in 0..access::FREE_TRIES + 1 {
            last = status(format!("/api/recordings/{a}/live?key={}", "0".repeat(64)), "127.0.0.2").await;
        }
        assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            status(format!("/api/recordings/{a}/live?key={ka}"), "127.0.0.2").await,
            StatusCode::TOO_MANY_REQUESTS,
            "even the right key waits"
        );
        assert!(s.access.key_allowed("127.0.0.3".parse().unwrap()).is_ok());
    }

    #[tokio::test]
    async fn a_strm_keeps_its_key_and_follows_the_address() {
        let s = server("strm-ensure");
        strm_mode(&s);
        let dir = scratch("ensure");
        let base = "Film - 2026-11-03 20.00";
        let id = recording(&s, &dir, base, RecordingState::Recording);
        // The same airing on another channel has the name already.
        let other = recording(&s, &dir, base, RecordingState::Recording);
        ensure(&s, other).unwrap();
        ensure(&s, id).unwrap();
        let (key, path) = strm_of(&s, id);
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            format!("[RECORDING] {base} (2).strm")
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("http://127.0.0.1:{}/api/recordings/{id}/live?key={key}\n", s.config().port)
        );
        // Again, or after a restart: the same file, the same key.
        ensure(&s, id).unwrap();
        assert_eq!(strm_of(&s, id), (key.clone(), path.clone()));
        // A new address is written in, with the key kept.
        let mut c = s.config();
        c.strm_host = "nas.local".into();
        s.replace_config(c);
        capture(&s, id);
        refresh(&s);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("http://nas.local:"), "{text}");
        assert_eq!(key_in(&text, id), Some(key.as_str()));
        // A key edited into the file by hand is not the one on record: a
        // new one replaces it.
        std::fs::write(&path, url("http://x:1", id, &"0".repeat(64))).unwrap();
        ensure(&s, id).unwrap();
        let (again, _) = strm_of(&s, id);
        assert_ne!(again, "0".repeat(64));
        assert_ne!(again, key);
        // Only the .strm is visible: nothing hidden or temporary is left.
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|n| n.ends_with(".strm") && !n.starts_with('.')), "{names:?}");
    }

    #[tokio::test]
    async fn starting_again_clears_the_strm_of_recordings_that_are_over() {
        let s = server("strm-recover");
        strm_mode(&s);
        let dir = scratch("recover");
        let on = recording(&s, &dir, "On - 2026-11-03 20.00", RecordingState::Recording);
        let done = recording(&s, &dir, "Done - 2026-11-03 20.00", RecordingState::Recording);
        let failed = recording(&s, &dir, "Failed - 2026-11-03 20.00", RecordingState::Recording);
        for id in [on, done, failed] {
            ensure(&s, id).unwrap();
        }
        let (key, on_path) = strm_of(&s, on);
        let (_, done_path) = strm_of(&s, done);
        let (_, failed_path) = strm_of(&s, failed);
        // The service stopped hard: these two finished in another way (or
        // were finished while their .strm could not be removed).
        s.db(|c| store::set_state(c, done, RecordingState::Done, None)).unwrap();
        s.db(|c| store::set_state(c, failed, RecordingState::Failed, Some("x"))).unwrap();
        // It starts again, reached at another address now.
        let mut c = s.config();
        c.strm_host = "media.local".into();
        s.replace_config(c);
        recover(&s).unwrap();
        assert!(!done_path.exists() && !failed_path.exists());
        let text = std::fs::read_to_string(&on_path).unwrap();
        assert!(text.starts_with("http://media.local:"), "{text}");
        assert_eq!(key_in(&text, on), Some(key.as_str()), "the key carries on");
        assert_eq!(s.db(store::with_strm).unwrap(), vec![(on, on_path.to_string_lossy().into_owned())]);
        // Their keys say "over", not "wrong".
        let k = s.db(|c| store::live_key(c, done)).unwrap();
        assert!(k.unwrap().finished.is_some());
        // Forgetting a recording takes its .strm along.
        forget(&s, on);
        assert!(!on_path.exists());
    }
}
