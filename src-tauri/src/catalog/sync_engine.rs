//! The background refresh.
//!
//! The only thing in the process that touches the network. Startup does not
//! wait on it; the UI paints from the mirror and updates as each phase lands.

use anyhow::{Context, Result};
use std::future::Future;
use std::io::{BufReader, Read};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::catalog::credentials::Account;
use crate::catalog::playlist::{self, PlaylistStats, PlaylistWriter};
use crate::catalog::{db, index, origin, prune, status, sync, vod_sync};
use crate::ipc::types::{SyncPhase, SyncStatusDto};
use crate::m3u::fetch;
use crate::source::{M3uSource, Refused};
use crate::xtream::XtreamClient;

/// Where a pass writes.
///
/// The application's mirror, always, except in tests: a playlist write
/// soft-deletes every channel it did not see, so a test that ran a whole
/// pass against the shared test database would empty it under every other
/// test. The passes that take one of these can be run end to end against a
/// database of their own.
pub(crate) trait Mirror: Clone + Send + Sync + 'static {
    fn write<R>(&self, f: impl FnOnce(&rusqlite::Connection) -> Result<R>) -> Result<R>;
    /// Rebuilds the search index from the channels just written. Returns
    /// how many are indexed.
    fn reindex(&self) -> Result<usize>;
}

/// The mirror on disk, through the sync's own writer connection.
#[derive(Clone, Copy)]
pub(crate) struct AppMirror;

impl Mirror for AppMirror {
    fn write<R>(&self, f: impl FnOnce(&rusqlite::Connection) -> Result<R>) -> Result<R> {
        db::with_write_db(f)
    }

    fn reindex(&self) -> Result<usize> {
        // Movies too: a tuner or Tvheadend write soft-deletes the movies of
        // the account it replaced, and the movie index must not keep them.
        db::with_write_db(index::rebuild_movies)?;
        db::with_write_db(index::rebuild)
    }
}

/// Where a phase's status goes. A trait so the phase sequencing can be tested
/// without a Tauri application to emit from.
pub trait StatusSink {
    fn publish(&self, status: SyncStatusDto);
}

#[cfg(feature = "gui")]
impl StatusSink for tauri::AppHandle {
    fn publish(&self, status: SyncStatusDto) {
        use tauri::Emitter;
        // A window that has gone away is not a sync failure.
        if let Err(e) = self.emit(crate::ipc::types::events::SYNC_STATUS, status) {
            tracing::debug!("sync status not delivered: {e}");
        }
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn dto(
    phase: SyncPhase,
    message: impl Into<String>,
    channels_total: i64,
    last_synced: Option<i64>,
) -> SyncStatusDto {
    SyncStatusDto {
        phase,
        message: message.into(),
        channels_total,
        last_synced,
    }
}

/// Strips credentials out of anything on its way to the UI or a log.
///
/// `reqwest` puts the failing URL in its `Display`, and every Xtream URL
/// carries `username` and `password` in the query string. The host and the
/// reason are worth keeping; the values are not.
///
/// Playlist and guide URLs put the account in more places — `user:pass@`
/// userinfo, `token=` and friends — so the work is
/// [`crate::m3u::redact`]'s, which knows all of them. (Those requests also
/// drop the URL from their errors outright; this is the second line.)
///
/// The kinds added after those carry secrets of their own shapes, and the
/// same redaction covers them: an HDHomeRun's guide token travels as
/// `DeviceAuth=` (caught by `auth=`, keys being matched without regard to
/// case), a Tvheadend stream URL as `?auth=`, and a Digest header's
/// `response`, `nonce` and quoted `username` are keys too. A `Basic` header
/// has no key, so its token is found by the word in front of it.
pub fn scrub(text: &str) -> String {
    use crate::m3u::redact;
    redact::redact_basic(&redact::redact_query(&redact::redact_userinfo(text)))
}

fn publish<S: StatusSink>(sink: &S, phase: SyncPhase, message: impl Into<String>) {
    let s = dto(phase, message, status::channel_total(), status::last_synced());
    status::record(&s);
    sink.publish(s);
}

pub(crate) type Step<'a> = Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>;

/// The HTTP status a failure carries, if any layer of it came from the server.
fn http_status(e: &anyhow::Error) -> Option<u16> {
    e.chain()
        .find_map(|c| c.downcast_ref::<reqwest::Error>())
        .and_then(|r| r.status())
        .map(|s| s.as_u16())
        .or_else(|| e.chain().find_map(|c| c.downcast_ref::<HttpRefusal>()).map(|r| r.0))
        .or_else(|| e.chain().find_map(|c| c.downcast_ref::<Refused>()).map(|r| r.status))
}

/// A stand-in for a refusing response in tests, where building a real
/// `reqwest::Error` with a status is not possible.
#[derive(Debug)]
pub struct HttpRefusal(pub u16);
impl std::fmt::Display for HttpRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP status {}", self.0)
    }
}
impl std::error::Error for HttpRefusal {}

/// Whether the server refused us outright rather than failing to answer.
///
/// 401 and 403 are about the account, not the endpoint: every later phase
/// sends the same credentials and will be refused the same way, so asking
/// again only adds load on a server that has already said no — and some
/// servers ban an address that keeps knocking.
fn is_refusal(e: &anyhow::Error) -> bool {
    matches!(http_status(e), Some(401 | 403))
}

/// What a person should read when a phase fails.
///
/// A source that knows why it was refused ([`Refused`]) says so in its own
/// words, which name the fix; anything else that is a refusal gets the
/// general one.
pub(crate) fn failure_message(e: &anyhow::Error) -> String {
    if let Some(r) = e.chain().find_map(|c| c.downcast_ref::<Refused>()) {
        return scrub(&r.why);
    }
    match http_status(e) {
        Some(code @ (401 | 403)) => {
            format!("The server refused this account (HTTP {code}). Check Settings → Account.")
        }
        _ => scrub(&format!("{e:#}")),
    }
}

/// How a pass went, which decides how long until the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassOutcome {
    /// At least one phase landed.
    Refreshed,
    /// Nothing landed, but nothing said no either: a timeout, a 5xx.
    Failed,
    /// The server refused the credentials.
    Refused,
}

/// Runs the steps in order.
///
/// A failing phase emits `Failed` and is logged, but the ones after it still
/// run: a server that will not serve the guide should not also cost the channel
/// list. The exception is a refusal (401/403), which ends the pass:
/// the same credentials go with every phase.
///
/// When nothing succeeded the pass ends on `Failed` rather than `Idle`.
/// Ending on `Idle` would make a failing pass read as failure, then "Synced 3
/// days ago", then failure again on the next try — the toolbar flickering
/// between the two for as long as the server kept refusing.
pub(crate) async fn run_steps<'a, S: StatusSink>(
    sink: &S,
    steps: Vec<(SyncPhase, Step<'a>)>,
) -> PassOutcome {
    let total = steps.len();
    let mut ok = 0;
    let mut refused = false;
    for (phase, work) in steps {
        publish(sink, phase, format!("{phase:?}"));
        match work.await {
            Ok(message) => {
                ok += 1;
                publish(sink, phase, message);
            }
            Err(e) => {
                tracing::warn!("sync phase {phase:?} failed: {}", scrub(&format!("{e:#}")));
                publish(sink, SyncPhase::Failed, failure_message(&e));
                if is_refusal(&e) {
                    refused = true;
                    break;
                }
            }
        }
    }
    if ok > 0 {
        status::mark_synced(now());
        publish(sink, SyncPhase::Idle, format!("{ok} of {total} phases refreshed"));
        PassOutcome::Refreshed
    } else if refused {
        PassOutcome::Refused
    } else {
        PassOutcome::Failed
    }
}

async fn step_categories(client: &XtreamClient, switched: bool) -> Result<String> {
    let cats = client.categories().await.context("fetching categories")?;
    let n = cats.len();
    db::with_write_db(|conn| land_categories(conn, &cats, switched))?;
    Ok(format!("{n} categories"))
}

/// A server's categories into the mirror. `switched` is [`origin::changed`]:
/// only then is anything the server did not list dropped, so a server that
/// has filled the mirror all along is synced exactly as before.
fn land_categories(
    conn: &rusqlite::Connection,
    cats: &[crate::xtream::Category],
    switched: bool,
) -> Result<()> {
    sync::sync_categories(conn, cats)?;
    if switched {
        let keep: Vec<&str> = cats.iter().map(|c| c.category_id.as_str()).collect();
        origin::prune_categories(conn, &keep)?;
    }
    Ok(())
}

/// A server's channels into the mirror, and the server noted as the mirror's
/// origin — after the channels, so a pass that pruned the old categories but
/// failed here prunes again next time rather than calling the swap done.
fn land_channels(
    conn: &rusqlite::Connection,
    chans: &[crate::xtream::Channel],
    origin: &str,
) -> Result<sync::SyncStats> {
    let stats = sync::sync_channels(conn, chans)?;
    origin::record(conn, origin)?;
    Ok(stats)
}

/// The provider's movies: both list calls, landed in one write, and the
/// movie index rebuilt.
///
/// The errors of those calls are `reqwest`'s, whose text includes the
/// request URL and so the account's username and password. They leave this
/// function scrubbed, because this phase's failure is read by people and
/// logs both.
///
/// The scrubbed error keeps the HTTP status it carried, so a 401 or 403 is
/// still a refusal and still gets the account's message.
///
/// `from` is the catalogue this pass is filling. The channels phase records
/// it as it lands; if that phase failed on an account switch the mirror
/// still holds the old account's catalogue, and landing the new account's
/// movies under it would mix the two. So the phase waits for a pass whose
/// channels landed.
async fn step_movies(client: &XtreamClient, from: &str) -> Result<String> {
    let fetched = async {
        let cats = client.vod_categories().await?;
        let streams = client.vod_streams().await?;
        Ok::<_, anyhow::Error>((cats, streams))
    }
    .await
    .map_err(|e| {
        let why = format!("fetching movies: {}", scrub(&format!("{e:#}")));
        match http_status(&e) {
            Some(code) => anyhow::Error::new(HttpRefusal(code)).context(why),
            None => anyhow::anyhow!(why),
        }
    })?;
    let (cats, streams) = fetched;
    if db::with_write_db(|conn| origin::changed(conn, from))? {
        anyhow::bail!("the channels have not been refreshed for this account yet; movies wait for them");
    }
    let movies: Vec<_> = streams.iter().map(vod_sync::NewMovie::from).collect();
    let stats = db::with_write_db(|conn| vod_sync::land_movies(conn, &cats, &movies, now()))?;
    let indexed = db::with_write_db(index::rebuild_movies)?;
    Ok(format!(
        "{indexed} movies ({} new, {} gone)",
        stats.inserted, stats.deleted
    ))
}

async fn step_channels(client: &XtreamClient, origin: &str) -> Result<String> {
    let chans = client.channels().await.context("fetching channels")?;
    let stats = db::with_write_db(|conn| land_channels(conn, &chans, origin))?;
    // Rebuilt here rather than as a phase of its own: it is local work, and
    // `SyncPhase` has no name for it because the UI has nothing to say.
    let indexed = db::with_write_db(index::rebuild)?;
    Ok(format!(
        "{indexed} channels ({} new, {} gone)",
        stats.inserted, stats.deleted
    ))
}

/// Feeds an async byte stream to a synchronous parser.
///
/// A guide can run to tens of megabytes; `parse_xmltv` wants a `BufRead`.
/// Bridging through a bounded channel means the HTTP side cannot run ahead of
/// the parser, so memory stays at a few chunks rather than the whole document.
struct ChunkReader {
    rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    pos: usize,
}

impl ChunkReader {
    fn new(rx: tokio::sync::mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            rx,
            chunk: Vec::new(),
            pos: 0,
        }
    }
}

impl Read for ChunkReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.pos >= self.chunk.len() {
            match self.rx.blocking_recv() {
                Some(next) => {
                    self.chunk = next;
                    self.pos = 0;
                }
                // The sender is gone: end of document, not an error.
                None => return Ok(0),
            }
        }
        let n = out.len().min(self.chunk.len() - self.pos);
        out[..n].copy_from_slice(&self.chunk[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

async fn step_epg(client: &XtreamClient) -> Result<String> {
    let resp = fetch::answer(client.xmltv_stream(), "the guide").await??;
    let programmes = land_xtream_guide(resp, AppMirror).await?;
    if programmes == 0 {
        // Not a failure the pass can name — the server sent a guide with
        // nothing in it — but the reason a guide is empty the next day.
        tracing::warn!("the server's guide had no programmes in it");
    }
    Ok(format!("{programmes} programmes"))
}

/// An Xtream server's guide into the mirror, as it downloads.
///
/// Through [`pump_into`], so the parse sees a download that stops halfway as
/// an error and its transaction rolls back, leaving the guide that was there;
/// and the phase waits for the parse to finish, so it reports what the parse
/// did. (Read to the end of whatever arrived and committed, a cut-short
/// guide would replace the programmes it reached and report the download's
/// error while the commit went ahead behind it.)
async fn land_xtream_guide<M: Mirror>(resp: reqwest::Response, mirror: M) -> Result<usize> {
    land_xtream_guide_within(resp, mirror, Deadlines::default()).await
}

async fn land_xtream_guide_within<M: Mirror>(
    resp: reqwest::Response,
    mirror: M,
    limits: Deadlines,
) -> Result<usize> {
    pump_into_within(resp, "the guide", None, limits, move |r| {
        mirror.write(|conn| {
            let n = crate::catalog::epg::parse_xmltv(BufReader::new(r), conn)?;
            // What the guide just landed on top of may be weeks old; see
            // `catalog::prune`.
            prune::prune_and_log(conn, now());
            Ok(n)
        })
    })
    .await
}

/// One pass at a time, however it was asked for.
///
/// The periodic loop and the toolbar's retry each start passes, and without
/// this a click during a pass would run two against the server at once.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Wakes the periodic loop early: a retry, or a changed account.
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();

struct RunningGuard;
impl Drop for RunningGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

/// Categories, then channels (and the index), then the guide, then movies.
///
/// Movies come last because live TV is what the app is for and the movie
/// list is the biggest download after the guide: a server that is slow or
/// refuses it should cost nothing that came before. They also land after the
/// channels have recorded which catalogue this is, which the movie list
/// compares with its own record to notice a change of account.
///
/// `None` when a pass was already running, so this one did not start: in
/// this process, or in another one sharing the mirror (the app and the
/// recording service on one machine; see [`super::sync_lease`]).
pub async fn run_once<S: StatusSink>(app: &S) -> Result<Option<PassOutcome>> {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(None);
    }
    let _running = RunningGuard;
    let Some(_lease) = super::sync_lease::take() else {
        return Ok(None);
    };
    let account = match crate::catalog::credentials::load_account() {
        Ok(a) => a,
        Err(e) => {
            // Scrubbed even here: the path is safe but the parse error may
            // not be, and this string reaches the UI.
            publish(app, SyncPhase::Failed, scrub(&format!("{e:#}")));
            return Err(e);
        }
    };
    let creds = match account {
        Account::Xtream(c) => c,
        Account::M3u(a) => {
            let src = M3uSource::new(a)?;
            return Ok(Some(run_m3u_pass(app, &src).await));
        }
        Account::HdHomeRun(a) => {
            let src = crate::hdhomerun::HdhrSource::new(a)?;
            return Ok(Some(
                crate::catalog::sync_devices::run_hdhomerun_pass(app, &src, AppMirror).await,
            ));
        }
        Account::Tvheadend(a) => {
            let src = crate::tvheadend::TvhSource::new(a)?;
            return Ok(Some(
                crate::catalog::sync_devices::run_tvheadend_pass(app, &src, AppMirror).await,
            ));
        }
    };
    // Which catalogue this is, and whether the mirror holds a different one.
    let from = origin::of_xtream(&creds);
    let switched = db::with_write_db(|conn| origin::changed(conn, &from))?;
    let client = XtreamClient::new(creds)?;
    // The server's time zone, for catch-up: not a phase, because the
    // catalogue does not depend on it and the toolbar has nothing to say.
    crate::catalog::panel_clock::learn(&client, &from).await;
    let outcome = run_steps(
        app,
        vec![
            (SyncPhase::Categories, Box::pin(step_categories(&client, switched))),
            (SyncPhase::Channels, Box::pin(step_channels(&client, &from))),
            (SyncPhase::Epg, Box::pin(step_epg(&client))),
            (SyncPhase::Movies, Box::pin(step_movies(&client, &from))),
        ],
    )
    .await;
    Ok(Some(outcome))
}

// ------------------------------------------------------------- M3U playlists

/// The same phases an Xtream server gets, so the toolbar says the same
/// things.
///
/// The playlist is downloaded once, in the categories phase, because one
/// document carries both categories and channels: fetching it twice to keep
/// the phases tidy would double a 60 MB download. The channels phase then
/// reports what landed and rebuilds the search index. If the download failed,
/// the channels phase fails with the same reason rather than claiming
/// success — a pass where nothing new arrived must not count as a refresh,
/// or the backoff would never apply. The movies phase does the same with the
/// movies the one document carried.
async fn run_m3u_pass<S: StatusSink>(sink: &S, src: &M3uSource) -> PassOutcome {
    let landed: Landed = std::sync::Mutex::new(None);
    let landed_movies: Landed = std::sync::Mutex::new(None);
    run_steps(
        sink,
        vec![
            (
                SyncPhase::Categories,
                Box::pin(step_m3u_playlist(src, &landed, &landed_movies)),
            ),
            (SyncPhase::Channels, Box::pin(step_m3u_channels(&landed))),
            (SyncPhase::Epg, Box::pin(step_m3u_epg(src))),
            (SyncPhase::Movies, Box::pin(step_landed_movies(&landed_movies))),
        ],
    )
    .await
}

/// What the playlist phase hands the channels phase: `(new, gone)`, or the
/// failure to repeat.
pub(crate) type Landed = std::sync::Mutex<Option<std::result::Result<(usize, usize), String>>>;

/// Records what a playlist-shaped write did, for the channels phase that
/// follows it.
pub(crate) fn hand_off(landed: &Landed, r: std::result::Result<&PlaylistStats, &anyhow::Error>) {
    let handoff = match r {
        Ok(s) => Ok((s.channels.inserted, s.channels.deleted)),
        Err(e) => Err(failure_message(e)),
    };
    *landed.lock().unwrap_or_else(|e| e.into_inner()) = Some(handoff);
}

async fn step_m3u_playlist(
    src: &M3uSource,
    landed: &Landed,
    landed_movies: &Landed,
) -> Result<String> {
    // A playlist's write replaces categories and soft-deletes unseen channels
    // whatever came before, so it needs no `switched`; it only has to say
    // that it is now the origin, for the server that may come after it. The
    // writer records that itself, before it lands the movies.
    let r = load_playlist(src).await;
    hand_off(landed, r.as_ref());
    hand_off_movies(landed_movies, r.as_ref());
    let s = r?;
    Ok(format!("{} categories", s.categories))
}

/// Records what a playlist-shaped write did to the movies, for the movies
/// phase that follows it.
pub(crate) fn hand_off_movies(
    landed: &Landed,
    r: std::result::Result<&PlaylistStats, &anyhow::Error>,
) {
    let handoff = match r {
        Ok(s) => Ok((s.movies.inserted, s.movies.deleted)),
        Err(e) => Err(failure_message(e)),
    };
    *landed.lock().unwrap_or_else(|e| e.into_inner()) = Some(handoff);
}

/// The movies phase of a playlist pass: what the write landed, and the movie
/// index rebuilt over it, or the write's failure repeated.
pub(crate) async fn step_landed_movies(landed: &Landed) -> Result<String> {
    let handoff = landed.lock().unwrap_or_else(|e| e.into_inner()).take();
    match handoff {
        Some(Ok((new, gone))) => {
            let indexed = db::with_write_db(index::rebuild_movies)?;
            Ok(format!("{indexed} movies ({new} new, {gone} gone)"))
        }
        Some(Err(why)) => Err(anyhow::anyhow!(why)),
        None => Err(anyhow::anyhow!("the playlist was not read")),
    }
}

async fn step_m3u_channels(landed: &Landed) -> Result<String> {
    step_landed_channels(&AppMirror, landed).await
}

/// The channels phase of any playlist-shaped pass: what the write before it
/// landed, and the index rebuilt over it.
pub(crate) async fn step_landed_channels<M: Mirror>(mirror: &M, landed: &Landed) -> Result<String> {
    let handoff = landed.lock().unwrap_or_else(|e| e.into_inner()).take();
    match handoff {
        Some(Ok((new, gone))) => {
            let indexed = mirror.reindex()?;
            Ok(format!("{indexed} channels ({new} new, {gone} gone)"))
        }
        Some(Err(why)) => Err(anyhow::anyhow!(why)),
        None => Err(anyhow::anyhow!("the playlist was not read")),
    }
}

async fn load_playlist(src: &M3uSource) -> Result<PlaylistStats> {
    let location = src.account().playlist.clone();
    match fetch::open(src.http(), &location, "the playlist").await? {
        fetch::Opened::Http(mut resp) => {
            let ct = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let Some(first) = fetch::next_chunk(&mut resp, "the playlist").await? else {
                anyhow::bail!("the playlist was empty");
            };
            if fetch::looks_like_stream(&first, ct.as_deref()) {
                // A stream, not a list. Stop downloading it.
                drop(resp);
                return tokio::task::spawn_blocking(move || {
                    db::with_write_db(|conn| write_single(conn, &location))
                })
                .await
                .context("joining the playlist writer")?;
            }
            pump_into(resp, "the playlist", Some(first), move |r| {
                db::with_write_db(|conn| write_playlist_from(conn, r, &location))
            })
            .await
        }
        fetch::Opened::File(f) => tokio::task::spawn_blocking(move || {
            db::with_write_db(|conn| write_playlist_from(conn, Box::new(f), &location))
        })
        .await
        .context("joining the playlist writer")?,
    }
}

/// One channel, for a "playlist" that turned out to be a stream.
fn write_single(conn: &rusqlite::Connection, location: &str) -> Result<PlaylistStats> {
    let mut w = PlaylistWriter::begin(conn)?;
    w.set_origin(origin::of_playlist(location));
    // Never classified: whatever its address ends in, the one URL an account
    // names is its channel.
    let e = crate::m3u::Entry {
        name: crate::m3u::parse::name_from_url(location),
        url: location.to_owned(),
        ..Default::default()
    };
    let identity = crate::m3u::ids::identity(&e);
    w.push_identified(e, &identity)?;
    w.finish(&crate::m3u::Header::default())
}

pub(crate) fn write_playlist_from(
    conn: &rusqlite::Connection,
    src: Box<dyn Read + Send>,
    location: &str,
) -> Result<PlaylistStats> {
    use std::io::BufRead;
    let mut r = fetch::decompressing(src)?;
    // A local file that is MPEG-TS; HTTP streams were caught by their
    // headers before getting here.
    if r.fill_buf()?.first() == Some(&0x47) {
        return write_single(conn, location);
    }
    let mut w = PlaylistWriter::begin(conn)?;
    w.set_origin(origin::of_playlist(location));
    let header = crate::m3u::parse::parse(r, |e| w.push(e))?;
    if header.hls {
        // An HLS playlist is one stream, however many segments it lists.
        // Dropping the writer rolls the segments back.
        drop(w);
        return write_single(conn, location);
    }
    w.finish(&header)
}

async fn step_m3u_epg(src: &M3uSource) -> Result<String> {
    let locations = src.guide_locations();
    if locations.is_empty() {
        return Ok("no guide configured".to_owned());
    }
    step_xmltv_guides(&AppMirror, src.http(), &locations).await
}

/// Loads XMLTV guides from `locations` (URLs or files) and matches the
/// mirror's stored-URL channels to them. The guide phase of a playlist, and
/// of an HDHomeRun whose account names its own guide.
pub(crate) async fn step_xmltv_guides<M: Mirror>(
    mirror: &M,
    http: &reqwest::Client,
    locations: &[String],
) -> Result<String> {
    let mut programmes = 0;
    let mut loaded = 0;
    let mut first_err = None;
    for loc in locations {
        // The first guide to load replaces the stored channel names; later
        // ones add to them, so two guides for two regions both count.
        match load_guide(mirror, http, loc, loaded == 0).await {
            Ok(n) => {
                programmes += n;
                loaded += 1;
            }
            Err(e) => {
                tracing::warn!("a guide failed: {}", scrub(&format!("{e:#}")));
                first_err.get_or_insert(e);
            }
        }
    }
    if loaded == 0 {
        // Every guide failed; the first reason stands for all of them, and
        // carries its status so a refusal is still recognised as one.
        return Err(first_err.unwrap_or_else(|| anyhow::anyhow!("no guide loaded")));
    }
    let (matched, total) = mirror.write(playlist::match_epg)?;
    // After matching: which channels use a guide channel decides how much
    // of its past is kept.
    prune_guide(mirror);
    let mut msg = format!("{programmes} programmes, guide for {matched} of {total} channels");
    if loaded < locations.len() {
        msg.push_str(&format!(" ({} of {} guides failed)", locations.len() - loaded, locations.len()));
    }
    Ok(msg)
}

async fn load_guide<M: Mirror>(
    mirror: &M,
    http: &reqwest::Client,
    location: &str,
    replace: bool,
) -> Result<usize> {
    let parse = guide_parser(mirror.clone(), replace);
    match fetch::open(http, location, "the guide").await? {
        fetch::Opened::Http(resp) => pump_into(resp, "the guide", None, parse).await,
        fetch::Opened::File(f) => tokio::task::spawn_blocking(move || parse(Box::new(f)))
            .await
            .context("joining the guide parser")?,
    }
}

/// What reads one XMLTV guide into the mirror, collecting its channel
/// names for matching. `replace` clears the names an earlier guide left.
pub(crate) fn guide_parser<M: Mirror>(
    mirror: M,
    replace: bool,
) -> impl FnOnce(Box<dyn Read + Send>) -> Result<usize> + Send + 'static {
    let names = crate::catalog::epg::ChannelNames::Collect { replace };
    move |r: Box<dyn Read + Send>| {
        mirror.write(|conn| {
            let r = fetch::decompressing(r)?;
            crate::catalog::epg::parse_xmltv_into(r, conn, names)
        })
    }
}

/// Drops the past programmes nothing can play any more, once a guide has
/// landed. Never fails the phase: see [`prune::prune_and_log`].
pub(crate) fn prune_guide<M: Mirror>(mirror: &M) {
    let _ = mirror.write(|conn| {
        prune::prune_and_log(conn, now());
        Ok(())
    });
}

/// A [`ChunkReader`] that turns an abandoned download into a read error.
///
/// Without it a connection that drops halfway looks, to the parser, exactly
/// like the end of the document — and a playlist cut off at channel 40,000
/// of 100,000 would commit, soft-deleting the other 60,000. The error makes
/// the parse fail and its transaction roll back instead.
struct Abortable {
    inner: ChunkReader,
    aborted: std::sync::Arc<AtomicBool>,
}

impl Read for Abortable {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(out)?;
        if n == 0 && !out.is_empty() && self.aborted.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("the download was cut short"));
        }
        Ok(n)
    }
}

/// How long one download may take in all, however steadily it trickles in.
///
/// [`fetch::STALL`] catches a server that stops; this catches one that
/// sends a byte every few seconds forever. Half an hour is many times what
/// a large guide takes on a slow line (tens of megabytes at a few hundred
/// kilobytes a second is a few minutes), and the pass after it is only
/// hours away.
pub(crate) const DOWNLOAD_CAP: Duration = Duration::from_secs(30 * 60);

/// What gives up on a download: no byte for `stall`, or `total` in all.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Deadlines {
    pub stall: Duration,
    pub total: Duration,
}

impl Default for Deadlines {
    fn default() -> Self {
        Self {
            stall: fetch::STALL,
            total: DOWNLOAD_CAP,
        }
    }
}

/// Streams a response into `work`, which runs on a blocking thread and
/// reads it as a `Read`. The same bridge as [`step_epg`], plus [`Abortable`].
///
/// A download that stalls ([`fetch::STALL`]) or runs past
/// [`DOWNLOAD_CAP`] fails like one cut short: the parse sees an error, its
/// transaction rolls back, and what was in the mirror stays.
pub(crate) async fn pump_into<T: Send + 'static>(
    resp: reqwest::Response,
    what: &'static str,
    first: Option<Vec<u8>>,
    work: impl FnOnce(Box<dyn Read + Send>) -> Result<T> + Send + 'static,
) -> Result<T> {
    pump_into_within(resp, what, first, Deadlines::default(), work).await
}

pub(crate) async fn pump_into_within<T: Send + 'static>(
    mut resp: reqwest::Response,
    what: &'static str,
    first: Option<Vec<u8>>,
    limits: Deadlines,
    work: impl FnOnce(Box<dyn Read + Send>) -> Result<T> + Send + 'static,
) -> Result<T> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
    let aborted = std::sync::Arc::new(AtomicBool::new(false));
    let reader = Abortable {
        inner: ChunkReader::new(rx),
        aborted: aborted.clone(),
    };
    let job = tokio::task::spawn_blocking(move || work(Box::new(reader)));
    let pump = async {
        if let Some(f) = first {
            if tx.send(f).await.is_err() {
                return Ok(());
            }
        }
        while let Some(chunk) = fetch::next_chunk_within(&mut resp, what, limits.stall).await? {
            // The parser having gone away is reported by the join below.
            if tx.send(chunk).await.is_err() {
                break;
            }
        }
        Ok(())
    };
    let pumped: Result<()> = match tokio::time::timeout(limits.total, pump).await {
        Ok(r) => r,
        Err(_) => Err(anyhow::anyhow!(
            "{what} took longer than {} minutes to download; giving up on it",
            limits.total.as_secs() / 60
        )),
    };
    if pumped.is_err() {
        // Before the sender drops, so the reader sees it at the end.
        aborted.store(true, Ordering::SeqCst);
    }
    drop(tx);
    let out = job.await.context("joining the parser")?;
    // The network's reason, when there is one, is the real one.
    pumped?;
    out
}

/// Asks for a pass now, through the periodic loop.
///
/// Going through the loop rather than spawning a pass of its own keeps every
/// pass on one schedule, so a retry also resets the backoff instead of
/// racing it.
pub fn request_now() {
    WAKE.notify_one();
}

// --------------------------------------------------- the other process

/// What [`watch_elsewhere`] noticed in one look.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Noticed {
    /// Another process saved a different account.
    pub account: bool,
    /// Another process finished a pass, so the catalogue is not what this
    /// one last said it was.
    pub catalogue: bool,
}

/// What the watcher saw last: the account file's fingerprint and when the
/// mirror last finished a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seen {
    account: Option<u64>,
    synced: Option<i64>,
}

impl Seen {
    pub fn now() -> Self {
        Self {
            account: crate::catalog::credentials::fingerprint(),
            synced: status::last_synced(),
        }
    }

    /// Compares `now` with what was seen, and remembers `now`.
    ///
    /// An account written by this process (`written_here`) is its own
    /// doing and was dealt with when it was saved. A pass is another
    /// process's when it finished later than this one's last
    /// (`synced_here`), and none of this one's is running (`own_pass`),
    /// which reports itself.
    pub fn look(
        &mut self,
        now: Seen,
        written_here: Option<u64>,
        synced_here: Option<i64>,
        own_pass: bool,
    ) -> Noticed {
        let account = now.account != self.account && now.account != written_here;
        let catalogue = now.synced != self.synced && now.synced > synced_here && !own_pass;
        *self = now;
        Noticed { account, catalogue }
    }
}

/// How often [`watch_elsewhere`] looks: a few seconds, so a change made in
/// a browser shows in the app about as soon as the browser shows it. Each
/// look is a read of a small file and one row.
const WATCH_EVERY: Duration = Duration::from_secs(5);

/// Notices what the other process sharing the account and the mirror does
/// (the app and the recording service, on one computer), which nothing else
/// tells this one about:
///
/// - **A new account** saved by the other one: `on_account` (the app drops
///   the client it keeps), and a pass here, since the other one may not be
///   syncing at all (`zapper-server --no-sync`). If it is, the lease lets
///   one pass run and the other finds the mirror fresh.
/// - **A pass it finished**: the status is published again, with the new
///   `lastSynced` and channel count, which is what makes a window read the
///   catalogue again (src/logic/catalogueRefresh.ts).
pub async fn watch_elsewhere<S: StatusSink>(sink: S, on_account: impl Fn()) {
    let mut seen = Seen::now();
    loop {
        tokio::time::sleep(WATCH_EVERY).await;
        let now = tokio::task::spawn_blocking(Seen::now).await.unwrap_or(seen);
        let noticed = seen.look(
            now,
            crate::catalog::credentials::written_here(),
            status::synced_here(),
            RUNNING.load(Ordering::SeqCst),
        );
        if noticed.account {
            tracing::info!("the account was changed by another process; syncing");
            on_account();
            request_now();
        }
        if noticed.catalogue {
            tracing::info!("the catalogue was refreshed by another process");
            let s = status::snapshot();
            let s = dto(SyncPhase::Idle, s.message, s.channels_total, s.last_synced);
            status::record(&s);
            sink.publish(s);
        }
    }
}

/// [`watch_elsewhere`] on the app's runtime, telling its window.
#[cfg(feature = "gui")]
pub fn spawn_watch_elsewhere(app: tauri::AppHandle, on_account: impl Fn() + Send + 'static) {
    tauri::async_runtime::spawn(watch_elsewhere(app, on_account));
}

/// Fire and forget, for callers with no periodic loop (the harnesses).
#[cfg(feature = "gui")]
pub fn spawn(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_once(&app).await {
            tracing::warn!("sync aborted: {}", scrub(&format!("{e:#}")));
        }
    });
}

/// Clamp for the configured interval.
///
/// The guide alone can be tens of megabytes, so a runaway setting could
/// hammer the server; and a very long one leaves the mirror stale for days.
/// Neither extreme is worth honouring literally.
const MIN_SYNC_INTERVAL_HOURS: u32 = 1;
const MAX_SYNC_INTERVAL_HOURS: u32 = 48;

pub fn interval_from_settings(hours: u32) -> std::time::Duration {
    let h = hours.clamp(MIN_SYNC_INTERVAL_HOURS, MAX_SYNC_INTERVAL_HOURS);
    std::time::Duration::from_secs(u64::from(h) * 3600)
}

/// How long to wait before the next pass, given when the last one finished.
///
/// The mirror is on disk and `last_synced` is stored beside it, so a restart
/// knows how old the catalogue is. Without this the loop would sync
/// immediately on every launch — re-fetching the catalogue and a guide of tens
/// of megabytes that was already there, minutes after the last time — and
/// only then start honouring the interval: opening the app four times in an
/// evening would mean four full syncs.
///
/// Zero means "now": never synced, or the mirror is older than the interval.
/// A `last_synced` in the future is treated the same way, because the only
/// ways to get one are a clock that has moved backwards or a copied
/// database, and both are better answered by syncing than by waiting days.
pub fn wait_before_next(
    last_synced: Option<i64>,
    now: i64,
    interval: std::time::Duration,
) -> std::time::Duration {
    let Some(last) = last_synced else {
        return std::time::Duration::ZERO;
    };
    if last > now {
        return std::time::Duration::ZERO;
    }
    let age = std::time::Duration::from_secs(now.saturating_sub(last).unsigned_abs());
    interval.saturating_sub(age)
}

/// How long to wait after the `n`th consecutive failed pass (1-based).
///
/// A failure leaves `last_synced` alone, so without a wait the loop finds the
/// mirror still stale and goes straight back to the server: a refusing
/// server would be asked again as fast as it could answer, with the toolbar
/// flickering through every phase each time.
///
/// Transient failures start at a minute and double to an hour. A refusal
/// starts at fifteen minutes: the credentials will not fix themselves, and
/// saving new ones or pressing retry wakes the loop at once anyway.
pub fn backoff(consecutive_failures: u32, refused: bool) -> Duration {
    let base: u64 = if refused { 15 * 60 } else { 60 };
    let cap: u64 = 60 * 60;
    let shift = consecutive_failures.saturating_sub(1).min(16);
    Duration::from_secs(base.saturating_mul(1u64 << shift).min(cap))
}

/// Sleeps for `d`, or until someone asks for a pass. True if woken early.
async fn wait_or_wake(d: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(d) => false,
        _ = WAKE.notified() => true,
    }
}

/// Syncs when the mirror is due, then again on the configured interval.
///
/// The interval is re-read each cycle rather than captured once, so changing
/// it in settings takes effect without a restart. Failed passes back off
/// (see [`backoff`]); `request_now` cuts any wait short.
#[cfg(feature = "gui")]
pub fn spawn_periodic(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(periodic(app));
}

/// The loop [`spawn_periodic`] runs, for a caller with its own runtime (the
/// recording service).
pub async fn periodic<S: StatusSink>(app: S) {
    // Before the first pass, so a vacuum never meets a sync's writes; and
    // whether or not one runs soon, so a mirror that last synced a month ago
    // is not searched with a month of finished programmes.
    if let Err(e) = tokio::task::spawn_blocking(|| prune::maintain(now())).await {
        tracing::warn!("guide housekeeping did not finish: {e}");
    }
    let mut failures: u32 = 0;
    // Set when the loop was woken on purpose: run now, whatever the
    // schedule says.
    let mut forced = false;
    loop {
        if !forced && failures == 0 {
            let hours = crate::catalog::settings::load_or_default().sync_interval_hours;
            let interval = interval_from_settings(hours);
            let wait = wait_before_next(crate::catalog::status::last_synced(), now(), interval);
            if !wait.is_zero() {
                tracing::info!(
                    "mirror is current; next sync in {} minutes",
                    wait.as_secs() / 60
                );
                forced = wait_or_wake(wait).await;
                continue;
            }
        }
        forced = false;

        let refused = match run_once(&app).await {
            Ok(Some(PassOutcome::Refreshed)) => {
                failures = 0;
                continue;
            }
            // A harness's pass is running; let it finish.
            Ok(None) => {
                forced = wait_or_wake(Duration::from_secs(5)).await;
                continue;
            }
            Ok(Some(PassOutcome::Refused)) => true,
            Ok(Some(PassOutcome::Failed)) => false,
            Err(e) => {
                // No usable credentials at all. Nothing will change until
                // some are saved, which wakes the loop.
                tracing::warn!("sync aborted: {}", scrub(&format!("{e:#}")));
                true
            }
        };
        failures = failures.saturating_add(1);
        let wait = backoff(failures, refused);
        tracing::info!("sync failed; retrying in {} minutes", wait.as_secs() / 60);
        if wait_or_wake(wait).await {
            // Asked for explicitly — a retry, or new credentials — so the
            // slate is clean and the pass runs now.
            failures = 0;
            forced = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Default)]
    struct Recorder(std::sync::Mutex<Vec<SyncStatusDto>>);

    impl StatusSink for Recorder {
        fn publish(&self, s: SyncStatusDto) {
            self.0.lock().unwrap().push(s);
        }
    }

    impl Recorder {
        fn phases(&self) -> Vec<SyncPhase> {
            self.0.lock().unwrap().iter().map(|s| s.phase).collect()
        }
        fn messages(&self) -> Vec<String> {
            self.0.lock().unwrap().iter().map(|s| s.message.clone()).collect()
        }
    }

    #[test]
    fn credentials_never_reach_a_status_message() {
        let raw = "error sending request for url \
                   (http://xtream.example/xmltv.php?username=bob&password=hunter2): timed out";
        let safe = scrub(raw);
        assert!(!safe.contains("bob"), "username leaked: {safe}");
        assert!(!safe.contains("hunter2"), "password leaked: {safe}");
        assert!(safe.contains("xtream.example"), "host is useful, keep it: {safe}");
        assert!(safe.contains("timed out"), "reason is useful, keep it: {safe}");
    }

    #[test]
    fn playlist_credentials_never_reach_a_status_message() {
        for raw in [
            "error sending request for url (http://bob:hunter2@cdn.example/list.m3u): timed out",
            "error decoding response body for url (http://p.example/get.php?username=bob&password=hunter2&type=m3u_plus)",
            "HTTP status 403 for url (https://cdn.example/x.m3u?token=hunter2)",
        ] {
            let safe = scrub(raw);
            assert!(!safe.contains("hunter2"), "secret leaked: {safe}");
            assert!(!safe.contains("bob"), "user leaked: {safe}");
            assert!(safe.contains(".example"), "host is useful, keep it: {safe}");
        }
    }

    fn gz(data: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    // These write to a connection of their own, never the shared test
    // database: a playlist write soft-deletes every channel it did not see,
    // which would pull the floor out from under every other test.

    #[test]
    fn a_gzipped_playlist_is_read_like_a_plain_one() {
        let c = crate::catalog::open_in_memory().unwrap();
        let body = gz(include_bytes!("../../tests/fixtures/sample.m3u"));
        let s = write_playlist_from(&c, Box::new(std::io::Cursor::new(body)), "x").unwrap();
        assert_eq!(s.channels.inserted, 8);
    }

    #[test]
    fn a_stream_or_an_hls_playlist_becomes_one_channel() {
        let c = crate::catalog::open_in_memory().unwrap();
        let ts = vec![0x47u8, 0x40, 0x11, 0x10];
        let s = write_playlist_from(&c, Box::new(std::io::Cursor::new(ts)), "http://cdn.example/live/news.ts")
            .unwrap();
        assert_eq!(s.channels.inserted, 1);
        let name: String = c
            .query_row("SELECT name FROM channels WHERE deleted_at IS NULL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "news");

        let hls = b"#EXTM3U\n#EXT-X-VERSION:3\n#EXTINF:6.0,\nseg1.ts\n#EXTINF:6.0,\nseg2.ts\n".to_vec();
        let url = "https://cdn.example/arte/index.m3u8";
        write_playlist_from(&c, Box::new(std::io::Cursor::new(hls)), url).unwrap();
        let urls: Vec<String> = c
            .prepare("SELECT stream_url FROM channels WHERE deleted_at IS NULL")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(urls, [url]);
    }

    /// What the rail would show: category ids, and the live channels' ids.
    fn mirror(c: &rusqlite::Connection) -> (Vec<String>, Vec<i64>) {
        let cats = c
            .prepare("SELECT id FROM categories ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let chans = c
            .prepare("SELECT stream_id FROM channels WHERE deleted_at IS NULL ORDER BY stream_id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        (cats, chans)
    }

    /// One Xtream pass, through the same functions `run_once` uses.
    fn xtream_pass(c: &rusqlite::Connection, from: &str) {
        let cats: Vec<crate::xtream::Category> = serde_json::from_str(
            r#"[{"category_id":"1","category_name":"News"},
                {"category_id":"2","category_name":"Sport"}]"#,
        )
        .unwrap();
        let chans: Vec<crate::xtream::Channel> = serde_json::from_str(
            r#"[{"stream_id":101,"name":"Northwind News","category_id":"1"},
                {"stream_id":102,"name":"Cobalt Sports","category_id":"2"}]"#,
        )
        .unwrap();
        let switched = origin::changed(c, from).unwrap();
        land_categories(c, &cats, switched).unwrap();
        land_channels(c, &chans, from).unwrap();
    }

    /// One playlist pass: the write, which records the origin itself, as `step_m3u_playlist`.
    fn playlist_pass(c: &rusqlite::Connection, location: &str) {
        let mut w = PlaylistWriter::begin(c).unwrap();
        let fixture = include_str!("../../tests/fixtures/sample.m3u");
        let header = crate::m3u::parse::parse(fixture.as_bytes(), |e| w.push(e)).unwrap();
        w.finish(&header).unwrap();
        origin::record(c, &origin::of_playlist(location)).unwrap();
    }

    #[test]
    fn xtream_to_playlist_to_xtream_leaves_nothing_of_the_playlist_behind() {
        let c = crate::catalog::open_in_memory().unwrap();
        let server = origin::of_xtream(&crate::xtream::Credentials {
            host: "http://p.example".into(),
            username: "me".into(),
            password: "pw".into(),
        });

        xtream_pass(&c, &server);
        let xtream_mirror = mirror(&c);
        assert_eq!(xtream_mirror, (vec!["1".into(), "2".into()], vec![101, 102]));
        crate::catalog::store::record_activity(&c, 101, 7200, true, 1_000).unwrap();

        playlist_pass(&c, "http://cdn.example/list.m3u?token=a");
        let (cats, chans) = mirror(&c);
        assert!(cats.iter().all(|id| id.starts_with("m3u-")), "{cats:?}");
        assert!(!chans.contains(&101) && !chans.contains(&102));
        // The Xtream favourite is hidden while its channel is gone…
        let watched = crate::catalog::store::watched(&c).unwrap();
        assert!(!watched.iter().any(|f| f.channel.stream_id == 101));

        xtream_pass(&c, &server);
        // …and nothing of the playlist is left: no empty groups in the rail.
        assert_eq!(mirror(&c), xtream_mirror);
        // The history was kept, not forgotten, and comes back with the channel.
        let watched = crate::catalog::store::watched(&c).unwrap();
        let news = watched.iter().find(|f| f.channel.stream_id == 101).unwrap();
        assert_eq!(news.watch_seconds, 7200);
    }

    fn live_movies(c: &rusqlite::Connection) -> i64 {
        c.query_row(
            "SELECT COUNT(*) FROM movies WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// An Xtream pass as `run_once` orders it: the channels record the
    /// origin, then the movies land.
    fn xtream_movies_pass(c: &rusqlite::Connection, from: &str, movies: &[vod_sync::NewMovie]) {
        xtream_pass(c, from);
        vod_sync::land_movies(c, &[], movies, 1_000).unwrap();
    }

    fn film(id: i64) -> vod_sync::NewMovie {
        vod_sync::NewMovie {
            movie_id: id,
            name: format!("Film {id}"),
            ..Default::default()
        }
    }

    #[test]
    fn switching_accounts_empties_the_movies_through_the_real_sync_path() {
        let c = crate::catalog::open_in_memory().unwrap();
        xtream_movies_pass(&c, "xtream:000000000000000a", &[film(1), film(2)]);
        assert_eq!(live_movies(&c), 2);
        // The same account's empty answer is a bad minute, and refused.
        assert!(vod_sync::land_movies(&c, &[], &[], 2_000).is_err());
        assert_eq!(live_movies(&c), 2);
        // Another account, with none.
        xtream_movies_pass(&c, "xtream:000000000000000b", &[]);
        assert_eq!(live_movies(&c), 0);
    }

    #[test]
    fn switching_to_a_playlist_without_movies_empties_them_in_that_write() {
        let c = crate::catalog::open_in_memory().unwrap();
        xtream_movies_pass(&c, "xtream:000000000000000a", &[film(1)]);
        let fixture = include_str!("../../tests/fixtures/sample.m3u")
            .as_bytes()
            .to_vec();
        write_playlist_from(
            &c,
            Box::new(std::io::Cursor::new(fixture)),
            "http://cdn.example/list.m3u",
        )
        .unwrap();
        assert_eq!(live_movies(&c), 0);
        // And the movies' own record moved with the catalogue's.
        let recorded: String = c
            .query_row(
                "SELECT value FROM meta WHERE key = 'movies_origin'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(Some(recorded), origin::recorded(&c).unwrap());
    }

    #[test]
    fn a_server_that_filled_the_mirror_all_along_is_synced_as_before() {
        // No pruning on an unchanged origin: a category the server stopped
        // listing stays, exactly as `sync_categories` always left it.
        let c = crate::catalog::open_in_memory().unwrap();
        let server = "xtream:0000000000000001";
        xtream_pass(&c, server);
        c.execute("INSERT INTO categories (id, name) VALUES ('9', 'Legacy')", [])
            .unwrap();
        xtream_pass(&c, server);
        assert!(mirror(&c).0.contains(&"9".to_owned()));
        // A different server is a different catalogue.
        xtream_pass(&c, "xtream:0000000000000002");
        assert!(!mirror(&c).0.contains(&"9".to_owned()));
    }

    #[test]
    fn a_different_playlist_replaces_the_last_ones_groups() {
        let c = crate::catalog::open_in_memory().unwrap();
        playlist_pass(&c, "/home/me/a.m3u");
        let mut w = PlaylistWriter::begin(&c).unwrap();
        let header = crate::m3u::parse::parse(
            "#EXTM3U\n#EXTINF:-1 group-title=\"Only\",Solo\nhttp://s/solo\n".as_bytes(),
            |e| w.push(e),
        )
        .unwrap();
        w.finish(&header).unwrap();
        let (cats, chans) = mirror(&c);
        assert_eq!(cats, [crate::m3u::ids::category_id("Only")]);
        assert_eq!(chans.len(), 1);
    }

    #[tokio::test]
    async fn a_download_cut_short_is_an_error_not_an_end_of_file() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1);
        let aborted = std::sync::Arc::new(AtomicBool::new(false));
        let mut r = Abortable {
            inner: ChunkReader::new(rx),
            aborted: aborted.clone(),
        };
        let joined = tokio::task::spawn_blocking(move || {
            let mut s = String::new();
            r.read_to_string(&mut s).map(|_| s)
        });
        tx.send(b"#EXTM3U\n".to_vec()).await.unwrap();
        aborted.store(true, Ordering::SeqCst);
        drop(tx);
        assert!(joined.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn the_channels_phase_repeats_a_failed_download_rather_than_succeeding() {
        let landed: Landed = std::sync::Mutex::new(Some(Err("the server refused the playlist".into())));
        let err = step_m3u_channels(&landed).await.unwrap_err().to_string();
        assert_eq!(err, "the server refused the playlist");
        let none: Landed = std::sync::Mutex::new(None);
        assert!(step_m3u_channels(&none).await.is_err());
    }

    #[tokio::test]
    async fn the_movies_phase_repeats_a_failed_download_rather_than_succeeding() {
        let landed: Landed =
            std::sync::Mutex::new(Some(Err("the server refused the playlist".into())));
        let err = step_landed_movies(&landed).await.unwrap_err().to_string();
        assert_eq!(err, "the server refused the playlist");
        let none: Landed = std::sync::Mutex::new(None);
        assert!(step_landed_movies(&none).await.is_err());
    }

    #[test]
    fn every_phase_gets_a_well_formed_status() {
        for phase in [
            SyncPhase::Idle,
            SyncPhase::Categories,
            SyncPhase::Channels,
            SyncPhase::Epg,
            SyncPhase::Movies,
            SyncPhase::Failed,
        ] {
            let s = dto(phase, "working", 60_000, Some(1_757_000_000));
            assert_eq!(s.phase, phase);
            assert_eq!(s.message, "working");
            assert_eq!(s.channels_total, 60_000);
            assert_eq!(s.last_synced, Some(1_757_000_000));
        }
    }

    // The guard is held across awaits deliberately: it serialises tests
    // against the process-wide sync status, which `publish` writes to, and no
    // production code ever waits on it.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_failing_phase_does_not_stop_the_ones_after_it() {
        let _guard = status::test_lock();
        db::install_test_db();
        let ran = Arc::new(AtomicUsize::new(0));
        let mark = |r: Arc<AtomicUsize>| async move {
            r.fetch_add(1, Ordering::SeqCst);
            Ok("fine".to_string())
        };

        let rec = Recorder::default();
        let ok = run_steps(
            &rec,
            vec![
                (SyncPhase::Categories, Box::pin(mark(ran.clone()))),
                (
                    SyncPhase::Channels,
                    Box::pin({
                        let r = ran.clone();
                        async move {
                            r.fetch_add(1, Ordering::SeqCst);
                            Err(anyhow::anyhow!("server said 599"))
                        }
                    }),
                ),
                (SyncPhase::Epg, Box::pin(mark(ran.clone()))),
            ],
        )
        .await;

        assert_eq!(ran.load(Ordering::SeqCst), 3, "a failure aborted later phases");
        assert_eq!(ok, PassOutcome::Refreshed);
        // Each phase announces itself before it runs — a large channel fetch
        // is slow enough that the UI needs to say so — and again with what it
        // did, or as Failed.
        assert_eq!(
            rec.phases(),
            [
                SyncPhase::Categories,
                SyncPhase::Categories,
                SyncPhase::Channels,
                SyncPhase::Failed,
                SyncPhase::Epg,
                SyncPhase::Epg,
                SyncPhase::Idle,
            ]
        );
        assert!(
            rec.messages().iter().any(|m| m.contains("server said 599")),
            "the failure reason was swallowed: {:?}",
            rec.messages()
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_failing_movies_phase_leaves_the_live_phases_landed() {
        let _guard = status::test_lock();
        db::install_test_db();
        let ok = || async { Ok("fine".to_string()) };
        let rec = Recorder::default();
        let outcome = run_steps(
            &rec,
            vec![
                (SyncPhase::Categories, Box::pin(ok())),
                (SyncPhase::Channels, Box::pin(ok())),
                (SyncPhase::Epg, Box::pin(ok())),
                (
                    SyncPhase::Movies,
                    Box::pin(async { Err(anyhow::anyhow!("the movie list was empty")) }),
                ),
            ],
        )
        .await;
        assert_eq!(outcome, PassOutcome::Refreshed);
        assert_eq!(
            rec.phases(),
            [
                SyncPhase::Categories,
                SyncPhase::Categories,
                SyncPhase::Channels,
                SyncPhase::Channels,
                SyncPhase::Epg,
                SyncPhase::Epg,
                SyncPhase::Movies,
                SyncPhase::Failed,
                SyncPhase::Idle,
            ]
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_movies_phase_the_server_refuses_never_shows_the_password() {
        let _guard = status::test_lock();
        db::install_test_db();
        let server = crate::test_http::serve(|_, _| crate::test_http::Reply::new(503, "busy"));
        let client = XtreamClient::new(crate::xtream::Credentials {
            host: server.base.clone(),
            username: "viewer-name".into(),
            password: "hunter2-secret".into(),
        })
        .unwrap();
        let rec = Recorder::default();
        let outcome = run_steps(
            &rec,
            vec![(SyncPhase::Movies, Box::pin(step_movies(&client, "xtream:0000000000000001")))],
        )
        .await;
        assert_eq!(outcome, PassOutcome::Failed);
        let shown = rec.messages().join("\n");
        assert!(shown.contains("503"), "{shown}");
        assert!(!shown.contains("hunter2-secret"), "{shown}");
        assert!(!shown.contains("viewer-name"), "{shown}");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_movies_phase_the_server_forbids_is_a_refusal() {
        let _guard = status::test_lock();
        db::install_test_db();
        let server = crate::test_http::serve(|_, _| crate::test_http::Reply::new(403, "no"));
        let client = XtreamClient::new(crate::xtream::Credentials {
            host: server.base.clone(),
            username: "viewer-name".into(),
            password: "hunter2-secret".into(),
        })
        .unwrap();
        let rec = Recorder::default();
        let outcome = run_steps(
            &rec,
            vec![(SyncPhase::Movies, Box::pin(step_movies(&client, "xtream:0000000000000001")))],
        )
        .await;
        assert_eq!(outcome, PassOutcome::Refused);
        let shown = rec.messages().join("\n");
        assert!(shown.contains("refused this account"), "{shown}");
        assert!(!shown.contains("hunter2-secret"), "{shown}");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn movies_wait_for_the_channels_of_an_account_just_switched_to() {
        let _guard = status::test_lock();
        db::install_test_db();
        let server = crate::test_http::serve(|_, _| crate::test_http::Reply::ok("[]"));
        let client = XtreamClient::new(crate::xtream::Credentials {
            host: server.base.clone(),
            username: "u".into(),
            password: "p".into(),
        })
        .unwrap();
        let before = db::with_write_db(|c| origin::recorded(c)).unwrap();
        db::with_write_db(|c| origin::record(c, "xtream:00000000000000aa")).unwrap();
        let err = step_movies(&client, "xtream:00000000000000bb").await;
        if let Some(b) = before {
            db::with_write_db(|c| origin::record(c, &b)).unwrap();
        }
        let err = err.unwrap_err().to_string();
        assert!(err.contains("movies wait"), "{err}");
    }

    #[test]
    fn a_single_stream_account_stays_a_channel_whatever_its_address_ends_in() {
        let c = crate::catalog::open_in_memory().unwrap();
        let s = write_single(&c, "http://cdn.example/clips/feed.mp4").unwrap();
        assert_eq!((s.channels.inserted, s.movies.inserted), (1, 0));
        assert_eq!(live_movies(&c), 0);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_refusal_ends_the_pass_and_stays_on_screen() {
        let _guard = status::test_lock();
        db::install_test_db();
        let ran = Arc::new(AtomicUsize::new(0));
        let refuse = |r: Arc<AtomicUsize>| async move {
            r.fetch_add(1, Ordering::SeqCst);
            Err::<String, _>(anyhow::Error::new(HttpRefusal(403)).context("fetching categories"))
        };

        let rec = Recorder::default();
        let outcome = run_steps(
            &rec,
            vec![
                (SyncPhase::Categories, Box::pin(refuse(ran.clone()))),
                (SyncPhase::Channels, Box::pin(refuse(ran.clone()))),
                (SyncPhase::Epg, Box::pin(refuse(ran.clone()))),
            ],
        )
        .await;

        assert_eq!(outcome, PassOutcome::Refused);
        // Same credentials for every phase: asking twice more is just load.
        assert_eq!(ran.load(Ordering::SeqCst), 1, "kept knocking after a 403");
        // Ends on Failed. A trailing Idle would flip the bar back to
        // "Synced …" and make a refusing server flicker the toolbar on every
        // retry.
        assert_eq!(rec.phases(), [SyncPhase::Categories, SyncPhase::Failed]);
        assert!(rec.messages().last().unwrap().contains("refused this account"));
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_pass_with_nothing_landed_is_a_failure() {
        let _guard = status::test_lock();
        db::install_test_db();
        let rec = Recorder::default();
        let outcome = run_steps(
            &rec,
            vec![(
                SyncPhase::Categories,
                Box::pin(async { Err::<String, _>(anyhow::anyhow!("timed out")) }),
            )],
        )
        .await;
        assert_eq!(outcome, PassOutcome::Failed);
        assert_eq!(rec.phases().last(), Some(&SyncPhase::Failed));
    }

    /// A mirror of its own, so a guide can be landed (or not) without the
    /// shared test database.
    #[derive(Clone)]
    struct OwnMirror(Arc<std::sync::Mutex<rusqlite::Connection>>);

    impl Mirror for OwnMirror {
        fn write<R>(&self, f: impl FnOnce(&rusqlite::Connection) -> Result<R>) -> Result<R> {
            f(&self.0.lock().unwrap())
        }
        fn reindex(&self) -> Result<usize> {
            Ok(0)
        }
    }

    /// Answers one request with `head` (status line and headers) and `body`,
    /// then hangs up: a `Content-Length` larger than the body is a download
    /// cut short.
    fn answer_once(head: String, body: Vec<u8>) -> String {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut req = [0u8; 4096];
            let _ = s.read(&mut req);
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(&body);
            let _ = s.flush();
            // Dropped: the connection closes with the body unfinished.
        });
        base
    }

    fn programme(channel: &str, hour: u32) -> String {
        format!(
            "<programme start=\"20990908{hour:02}0000 +0000\" stop=\"20990908{:02}0000 +0000\" \
             channel=\"{channel}\"><title>Hour {hour}</title></programme>\n",
            hour + 1
        )
    }

    #[tokio::test]
    async fn a_guide_cut_short_leaves_the_guide_that_was_there() {
        let mirror = OwnMirror(Arc::new(std::sync::Mutex::new(
            crate::catalog::open_in_memory().unwrap(),
        )));
        let count = |m: &OwnMirror| -> i64 {
            m.0.lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM epg", [], |r| r.get(0))
                .unwrap()
        };
        // Yesterday's guide, landed whole.
        let whole = format!("<tv>\n{}{}</tv>\n", programme("old.uk", 1), programme("old.uk", 2));
        let base = answer_once(
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", whole.len()),
            whole.into_bytes(),
        );
        let resp = reqwest::get(format!("{base}/xmltv.php")).await.unwrap();
        assert_eq!(land_xtream_guide(resp, mirror.clone()).await.unwrap(), 2);
        assert_eq!(count(&mirror), 2);

        // Today's, cut off between two programmes: what arrived parses, as
        // far as it goes, and must not be committed.
        let mut part = String::from("<tv>\n");
        for h in 3..9 {
            part.push_str(&programme("new.uk", h));
        }
        let base = answer_once(
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", part.len() * 50),
            part.into_bytes(),
        );
        let resp = reqwest::get(format!("{base}/xmltv.php")).await.unwrap();
        let err = land_xtream_guide(resp, mirror.clone()).await.unwrap_err();
        assert!(format!("{err:#}").contains("the guide"), "{err:#}");
        assert_eq!(count(&mirror), 2, "a guide cut short was committed");
    }

    /// Answers one request with `head` and then `body` a piece at a time,
    /// `gap` apart, then holds the connection open for `hold` without
    /// sending another byte: a server that stops without hanging up.
    fn answer_and_stall(head: String, pieces: Vec<Vec<u8>>, gap: Duration, hold: Duration) -> String {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut req = [0u8; 4096];
            let _ = s.read(&mut req);
            let _ = s.write_all(head.as_bytes());
            for p in pieces {
                if s.write_all(&p).and_then(|_| s.flush()).is_err() {
                    return;
                }
                std::thread::sleep(gap);
            }
            std::thread::sleep(hold);
        });
        base
    }

    fn mirror_with_yesterdays_guide() -> OwnMirror {
        let c = crate::catalog::open_in_memory().unwrap();
        let whole = format!("<tv>\n{}{}</tv>\n", programme("old.uk", 1), programme("old.uk", 2));
        crate::catalog::epg::parse_xmltv(whole.as_bytes(), &c).unwrap();
        OwnMirror(Arc::new(std::sync::Mutex::new(c)))
    }

    fn guide_rows(m: &OwnMirror) -> i64 {
        m.0.lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM epg", [], |r| r.get(0))
            .unwrap()
    }

    #[tokio::test]
    async fn a_guide_that_stops_arriving_fails_and_keeps_the_guide_that_was_there() {
        let mirror = mirror_with_yesterdays_guide();
        let mut part = String::from("<tv>\n");
        for h in 3..9 {
            part.push_str(&programme("new.uk", h));
        }
        // Half the body, then silence with the connection still open.
        let base = answer_and_stall(
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", part.len() * 50),
            vec![part.into_bytes()],
            Duration::ZERO,
            Duration::from_secs(5),
        );
        let resp = reqwest::get(format!("{base}/xmltv.php")).await.unwrap();
        let limits = Deadlines {
            stall: Duration::from_millis(300),
            total: Duration::from_secs(30),
        };
        let started = std::time::Instant::now();
        let err = land_xtream_guide_within(resp, mirror.clone(), limits)
            .await
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(3), "the stall was waited out");
        assert_eq!(
            format!("{err:#}"),
            "the server stopped sending the guide: nothing arrived for 0 s"
        );
        assert_eq!(guide_rows(&mirror), 2, "a stalled guide was committed");
    }

    #[tokio::test]
    async fn a_guide_that_only_trickles_in_is_given_up_on() {
        let mirror = mirror_with_yesterdays_guide();
        let body = format!("<tv>\n{}</tv>\n", programme("new.uk", 3)).into_bytes();
        let len = body.len();
        // A byte every 50 ms: never a stall, never done in time.
        let pieces = body.into_iter().map(|b| vec![b]).collect();
        let base = answer_and_stall(
            format!("HTTP/1.1 200 OK\r\nContent-Length: {len}\r\n\r\n"),
            pieces,
            Duration::from_millis(50),
            Duration::ZERO,
        );
        let resp = reqwest::get(format!("{base}/xmltv.php")).await.unwrap();
        let limits = Deadlines {
            stall: Duration::from_secs(2),
            total: Duration::from_millis(500),
        };
        let err = land_xtream_guide_within(resp, mirror.clone(), limits)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("the guide took longer than"), "{err:#}");
        assert_eq!(guide_rows(&mirror), 2);
    }

    #[tokio::test]
    async fn a_server_that_never_answers_is_given_up_on_by_its_first_chunk() {
        // Headers, then nothing: the playlist's first read is a chunk too.
        let base = answer_and_stall(
            "HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n".into(),
            Vec::new(),
            Duration::ZERO,
            Duration::from_secs(5),
        );
        let mut resp = reqwest::get(format!("{base}/list.m3u")).await.unwrap();
        let err = fetch::next_chunk_within(&mut resp, "the playlist", Duration::from_millis(200))
            .await
            .unwrap_err();
        assert!(
            err.to_string().starts_with("the server stopped sending the playlist"),
            "{err}"
        );
    }

    #[test]
    fn only_the_other_processs_changes_are_noticed() {
        let at = |account: Option<u64>, synced: Option<i64>| Seen { account, synced };
        let mut seen = at(Some(1), Some(100));
        // Nothing moved.
        assert_eq!(seen.look(at(Some(1), Some(100)), None, Some(100), false), Noticed::default());
        // This process saved the account: its own doing.
        assert_eq!(seen.look(at(Some(2), Some(100)), Some(2), Some(100), false), Noticed::default());
        // Another process saved one.
        let n = seen.look(at(Some(3), Some(100)), Some(2), Some(100), false);
        assert!(n.account && !n.catalogue);
        // ...and once seen, it is not news again.
        assert!(!seen.look(at(Some(3), Some(100)), Some(2), Some(100), false).account);
        // The file going away is a change too (an account removed by hand).
        assert!(seen.look(at(None, Some(100)), Some(2), Some(100), false).account);
        // A pass of this process's own, while it runs and after.
        assert!(!seen.look(at(None, Some(200)), None, Some(100), true).catalogue);
        assert!(!seen.look(at(None, Some(300)), None, Some(300), false).catalogue);
        // A pass of the other process's.
        let n = seen.look(at(None, Some(400)), None, Some(300), false);
        assert!(n.catalogue && !n.account);
        assert!(!seen.look(at(None, Some(400)), None, Some(300), false).catalogue);
    }

    #[test]
    fn failed_passes_back_off_rather_than_retrying_at_once() {
        // Transient: a minute, doubling, capped at an hour.
        assert_eq!(backoff(1, false), Duration::from_secs(60));
        assert_eq!(backoff(2, false), Duration::from_secs(120));
        assert_eq!(backoff(3, false), Duration::from_secs(240));
        assert_eq!(backoff(50, false), Duration::from_secs(3600));
        // Refused: starts long, same cap.
        assert_eq!(backoff(1, true), Duration::from_secs(15 * 60));
        assert_eq!(backoff(2, true), Duration::from_secs(30 * 60));
        assert_eq!(backoff(9, true), Duration::from_secs(3600));
        // Never zero, whatever the count.
        assert!(!backoff(0, false).is_zero());
    }

    #[tokio::test]
    async fn the_reader_streams_chunks_rather_than_collecting_them() {
        // Capacity one: the sender cannot run ahead, which is the whole point
        // for a guide of tens of megabytes.
        let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1);
        let joined = tokio::task::spawn_blocking(move || {
            let mut s = String::new();
            BufReader::new(ChunkReader::new(rx))
                .read_to_string(&mut s)
                .unwrap();
            s
        });
        for part in ["<tv>", "", "<programme/>", "</tv>"] {
            tx.send(part.as_bytes().to_vec()).await.unwrap();
        }
        drop(tx);
        assert_eq!(joined.await.unwrap(), "<tv><programme/></tv>");
    }

    #[test]
    fn a_fresh_mirror_is_not_re_fetched_on_launch() {
        let six_hours = interval_from_settings(6);
        let now = 1_757_000_000;

        // Synced a minute ago: wait almost the whole interval.
        let wait = wait_before_next(Some(now - 60), now, six_hours);
        assert_eq!(wait.as_secs(), 6 * 3600 - 60);

        // Synced five hours ago: one hour left.
        assert_eq!(
            wait_before_next(Some(now - 5 * 3600), now, six_hours).as_secs(),
            3600
        );
    }

    #[test]
    fn a_stale_or_missing_mirror_syncs_at_once() {
        let six_hours = interval_from_settings(6);
        let now = 1_757_000_000;

        // Never synced.
        assert!(wait_before_next(None, now, six_hours).is_zero());
        // Older than the interval.
        assert!(wait_before_next(Some(now - 7 * 3600), now, six_hours).is_zero());
        // Exactly due.
        assert!(wait_before_next(Some(now - 6 * 3600), now, six_hours).is_zero());
        // A timestamp from the future: a clock that went backwards, or a
        // database copied from another machine. Syncing beats waiting days.
        assert!(wait_before_next(Some(now + 90_000), now, six_hours).is_zero());
    }

    #[test]
    fn the_sync_interval_is_clamped_at_both_ends() {
        use std::time::Duration;
        // Zero would mean syncing continuously against a server whose guide
        // runs to tens of megabytes.
        assert_eq!(interval_from_settings(0), Duration::from_secs(3600));
        assert_eq!(interval_from_settings(6), Duration::from_secs(6 * 3600));
        assert_eq!(interval_from_settings(9_999), Duration::from_secs(48 * 3600));
    }
}
