//! The background refresh.
//!
//! The only thing in the process that touches the network. Startup does not
//! wait on it; the UI paints from the mirror and updates as each phase lands.

use anyhow::{Context, Result};
use std::future::Future;
use std::io::{BufReader, Read};
use std::pin::Pin;

use crate::catalog::{db, index, status, sync};
use crate::ipc::types::{events, SyncPhase, SyncStatusDto};
use crate::xtream::XtreamClient;


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
pub fn scrub(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(hit) = ["username=", "password=", "pass=", "user="]
        .iter()
        .filter_map(|k| rest.find(k).map(|i| (i, k.len())))
        .min_by_key(|(i, _)| *i)
    {
        let (at, key_len) = hit;
        let value_start = at + key_len;
        out.push_str(&rest[..value_start]);
        out.push_str("<redacted>");
        // A query value ends at the next separator; everything after it —
        // including the closing bracket reqwest wraps the URL in — stays.
        let end = rest[value_start..]
            .find(['&', ')', ' ', '"'])
            .map(|i| value_start + i)
            .unwrap_or(rest.len());
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn publish<S: StatusSink>(sink: &S, phase: SyncPhase, message: impl Into<String>) {
    let s = dto(phase, message, status::channel_total(), status::last_synced());
    status::record(&s);
    sink.publish(s);
}

type Step<'a> = Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>;

/// Runs every step, in order, whatever happens.
///
/// A failing phase emits `Failed` and is logged, but the ones after it still
/// run: a panel that will not serve the large guide should not also cost the
/// channel list. Returns how many phases succeeded.
async fn run_steps<'a, S: StatusSink>(sink: &S, steps: Vec<(SyncPhase, Step<'a>)>) -> usize {
    let mut ok = 0;
    for (phase, work) in steps {
        publish(sink, phase, format!("{phase:?}"));
        match work.await {
            Ok(message) => {
                ok += 1;
                publish(sink, phase, message);
            }
            Err(e) => {
                let message = scrub(&format!("{e:#}"));
                tracing::warn!("sync phase {phase:?} failed: {message}");
                publish(sink, SyncPhase::Failed, message);
            }
        }
    }
    if ok > 0 {
        status::mark_synced(now());
    }
    publish(sink, SyncPhase::Idle, format!("{ok} of 3 phases refreshed"));
    ok
}

async fn step_categories(client: &XtreamClient) -> Result<String> {
    let cats = client.categories().await.context("fetching categories")?;
    let n = cats.len();
    db::with_write_db(|conn| sync::sync_categories(conn, &cats))?;
    Ok(format!("{n} categories"))
}

async fn step_channels(client: &XtreamClient) -> Result<String> {
    let chans = client.channels().await.context("fetching channels")?;
    let stats = db::with_write_db(|conn| sync::sync_channels(conn, &chans))?;
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

/// Categories, then channels (and the index), then the guide.
pub async fn run_once(app: &tauri::AppHandle) -> Result<()> {
    let creds = match crate::catalog::credentials::load() {
        Ok(c) => c,
        Err(e) => {
            // Scrubbed even here: the path is safe but the parse error may
            // not be, and this string reaches the UI.
            publish(app, SyncPhase::Failed, scrub(&format!("{e:#}")));
            return Err(e);
        }
    };
    let client = XtreamClient::new(creds)?;
    run_steps(
        app,
        vec![
            (SyncPhase::Categories, Box::pin(step_categories(&client))),
            (SyncPhase::Channels, Box::pin(step_channels(&client))),
            (SyncPhase::Epg, Box::pin(step_epg(&client))),
        ],
    )
    .await;
    Ok(())
}

/// Fire and forget. Startup must never wait on this.
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

/// How long to wait before trying again after a failed pass.
///
/// A failure leaves `last_synced` alone, so without this the loop would find
/// the mirror still stale and retry immediately, forever, as fast as the
/// panel could refuse it.
const RETRY_AFTER_FAILURE: std::time::Duration = std::time::Duration::from_secs(300);

/// Syncs when the mirror is due, then again on the configured interval.
///
/// The interval is re-read each cycle rather than captured once, so changing
/// it in settings takes effect without a restart.
pub fn spawn_periodic(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let hours = crate::catalog::settings::load_or_default().sync_interval_hours;
            let interval = interval_from_settings(hours);
            let wait = wait_before_next(crate::catalog::status::last_synced(), now(), interval);
            if !wait.is_zero() {
                tracing::info!(
                    "mirror is current; next sync in {} minutes",
                    wait.as_secs() / 60
                );
                tokio::time::sleep(wait).await;
                continue;
            }

            if let Err(e) = run_once(&app).await {
                tracing::warn!("sync aborted: {}", scrub(&format!("{e:#}")));
                // `last_synced` is untouched by a failure, so the check above
                // would send us straight back here.
                tokio::time::sleep(RETRY_AFTER_FAILURE).await;
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
        assert_eq!(ok, 2);
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
