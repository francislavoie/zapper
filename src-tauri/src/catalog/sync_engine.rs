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
use crate::catalog::{db, index, origin, status, sync};
use crate::ipc::types::{events, SyncPhase, SyncStatusDto};
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
        db::with_write_db(index::rebuild)
    }
}

/// Where a phase's status goes. A trait so the phase sequencing can be tested
/// without a Tauri application to emit from.
pub trait StatusSink {
    fn publish(&self, status: SyncStatusDto);
}

impl StatusSink for tauri::AppHandle {
    fn publish(&self, status: SyncStatusDto) {
        use tauri::Emitter;
        // A window that has gone away is not a sync failure.
        if let Err(e) = self.emit(events::SYNC_STATUS, status) {
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
            format!("The panel refused this account (HTTP {code}). Check Settings → Account.")
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
    let mut resp = client.xmltv_stream().await?;

    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
    let parsing = tokio::task::spawn_blocking(move || {
        db::with_write_db(|conn| crate::catalog::epg::parse_xmltv(BufReader::new(ChunkReader::new(rx)), conn))
    });

    while let Some(chunk) = resp.chunk().await.context("reading the guide")? {
        // The parser having gone away is reported by the join below, which
        // carries the real reason; this send failing is only its shadow.
        if tx.send(chunk.to_vec()).await.is_err() {
            break;
        }
    }
    drop(tx);

    let programmes = parsing.await.context("joining the guide parser")??;
    Ok(format!("{programmes} programmes"))
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

/// Categories, then channels (and the index), then the guide.
///
/// `None` when a pass was already running, so this one did not start.
pub async fn run_once(app: &tauri::AppHandle) -> Result<Option<PassOutcome>> {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(None);
    }
    let _running = RunningGuard;
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
    let outcome = run_steps(
        app,
        vec![
            (SyncPhase::Categories, Box::pin(step_categories(&client, switched))),
            (SyncPhase::Channels, Box::pin(step_channels(&client, &from))),
            (SyncPhase::Epg, Box::pin(step_epg(&client))),
        ],
    )
    .await;
    Ok(Some(outcome))
}

// ------------------------------------------------------------- M3U playlists

/// The same three phases an Xtream server gets, so the toolbar says the same
/// things.
///
/// The playlist is downloaded once, in the categories phase, because one
/// document carries both categories and channels: fetching it twice to keep
/// the phases tidy would double a 60 MB download. The channels phase then
/// reports what landed and rebuilds the search index. If the download failed,
/// the channels phase fails with the same reason rather than claiming
/// success — a pass where nothing new arrived must not count as a refresh,
/// or the backoff would never apply.
async fn run_m3u_pass<S: StatusSink>(sink: &S, src: &M3uSource) -> PassOutcome {
    let landed: Landed = std::sync::Mutex::new(None);
    run_steps(
        sink,
        vec![
            (SyncPhase::Categories, Box::pin(step_m3u_playlist(src, &landed))),
            (SyncPhase::Channels, Box::pin(step_m3u_channels(&landed))),
            (SyncPhase::Epg, Box::pin(step_m3u_epg(src))),
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

async fn step_m3u_playlist(src: &M3uSource, landed: &Landed) -> Result<String> {
    let r = load_playlist(src).await;
    // A playlist's write replaces categories and soft-deletes unseen channels
    // whatever came before, so it needs no `switched`; it only has to say
    // that it is now the origin, for the server that may come after it.
    let r = r.and_then(|s| {
        let from = origin::of_playlist(&src.account().playlist);
        db::with_write_db(|conn| origin::record(conn, &from))?;
        Ok(s)
    });
    hand_off(landed, r.as_ref());
    let s = r?;
    Ok(format!("{} categories", s.categories))
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
    w.push(crate::m3u::Entry {
        name: crate::m3u::parse::name_from_url(location),
        url: location.to_owned(),
        ..Default::default()
    })?;
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

/// Streams a response into `work`, which runs on a blocking thread and
/// reads it as a `Read`. The same bridge as [`step_epg`], plus [`Abortable`].
pub(crate) async fn pump_into<T: Send + 'static>(
    mut resp: reqwest::Response,
    what: &'static str,
    first: Option<Vec<u8>>,
    work: impl FnOnce(Box<dyn Read + Send>) -> Result<T> + Send + 'static,
) -> Result<T> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
    let aborted = std::sync::Arc::new(AtomicBool::new(false));
    let reader = Abortable {
        inner: ChunkReader::new(rx),
        aborted: aborted.clone(),
    };
    let job = tokio::task::spawn_blocking(move || work(Box::new(reader)));
    let pumped: Result<()> = async {
        if let Some(f) = first {
            if tx.send(f).await.is_err() {
                return Ok(());
            }
        }
        while let Some(chunk) = fetch::next_chunk(&mut resp, what).await? {
            // The parser having gone away is reported by the join below.
            if tx.send(chunk).await.is_err() {
                break;
            }
        }
        Ok(())
    }
    .await;
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

/// Fire and forget, for callers with no periodic loop (the harnesses).
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
pub fn spawn_periodic(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
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
    });
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
                   (http://panel.example/xmltv.php?username=bob&password=hunter2): timed out";
        let safe = scrub(raw);
        assert!(!safe.contains("bob"), "username leaked: {safe}");
        assert!(!safe.contains("hunter2"), "password leaked: {safe}");
        assert!(safe.contains("panel.example"), "host is useful, keep it: {safe}");
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

    /// One panel pass, through the same functions `run_once` uses.
    fn panel_pass(c: &rusqlite::Connection, from: &str) {
        let cats: Vec<crate::xtream::Category> = serde_json::from_str(
            r#"[{"category_id":"1","category_name":"News"},
                {"category_id":"2","category_name":"Sport"}]"#,
        )
        .unwrap();
        let chans: Vec<crate::xtream::Channel> = serde_json::from_str(
            r#"[{"stream_id":101,"name":"NWN News","category_id":"1"},
                {"stream_id":102,"name":"Summit Sports","category_id":"2"}]"#,
        )
        .unwrap();
        let switched = origin::changed(c, from).unwrap();
        land_categories(c, &cats, switched).unwrap();
        land_channels(c, &chans, from).unwrap();
    }

    /// One playlist pass: the write, then the origin, as `step_m3u_playlist`.
    fn playlist_pass(c: &rusqlite::Connection, location: &str) {
        let mut w = PlaylistWriter::begin(c).unwrap();
        let fixture = include_str!("../../tests/fixtures/sample.m3u");
        let header = crate::m3u::parse::parse(fixture.as_bytes(), |e| w.push(e)).unwrap();
        w.finish(&header).unwrap();
        origin::record(c, &origin::of_playlist(location)).unwrap();
    }

    #[test]
    fn panel_to_playlist_to_panel_leaves_nothing_of_the_playlist_behind() {
        let c = crate::catalog::open_in_memory().unwrap();
        let panel = origin::of_xtream(&crate::xtream::Credentials {
            host: "http://p.example".into(),
            username: "me".into(),
            password: "pw".into(),
        });

        panel_pass(&c, &panel);
        let panel_mirror = mirror(&c);
        assert_eq!(panel_mirror, (vec!["1".into(), "2".into()], vec![101, 102]));
        crate::catalog::store::record_activity(&c, 101, 7200, true, 1_000).unwrap();

        playlist_pass(&c, "http://cdn.example/list.m3u?token=a");
        let (cats, chans) = mirror(&c);
        assert!(cats.iter().all(|id| id.starts_with("m3u-")), "{cats:?}");
        assert!(!chans.contains(&101) && !chans.contains(&102));
        // The Xtream favourite is hidden while its channel is gone…
        let watched = crate::catalog::store::watched(&c).unwrap();
        assert!(!watched.iter().any(|f| f.channel.stream_id == 101));

        panel_pass(&c, &panel);
        // …and nothing of the playlist is left: no empty groups in the rail.
        assert_eq!(mirror(&c), panel_mirror);
        // The history was kept, not forgotten, and comes back with the channel.
        let watched = crate::catalog::store::watched(&c).unwrap();
        let nwn = watched.iter().find(|f| f.channel.stream_id == 101).unwrap();
        assert_eq!(nwn.watch_seconds, 7200);
    }

    #[test]
    fn a_panel_that_filled_the_mirror_all_along_is_synced_as_before() {
        // No pruning on an unchanged origin: a category the panel stopped
        // listing stays, exactly as `sync_categories` always left it.
        let c = crate::catalog::open_in_memory().unwrap();
        let panel = "xtream:0000000000000001";
        panel_pass(&c, panel);
        c.execute("INSERT INTO categories (id, name) VALUES ('9', 'Legacy')", [])
            .unwrap();
        panel_pass(&c, panel);
        assert!(mirror(&c).0.contains(&"9".to_owned()));
        // A different panel is a different catalogue.
        panel_pass(&c, "xtream:0000000000000002");
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

    #[test]
    fn every_phase_gets_a_well_formed_status() {
        for phase in [
            SyncPhase::Idle,
            SyncPhase::Categories,
            SyncPhase::Channels,
            SyncPhase::Epg,
            SyncPhase::Failed,
        ] {
            let s = dto(phase, "working", 50_414, Some(1_757_000_000));
            assert_eq!(s.phase, phase);
            assert_eq!(s.message, "working");
            assert_eq!(s.channels_total, 50_414);
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
