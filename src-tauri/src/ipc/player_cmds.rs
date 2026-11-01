//! Playback control.
//!
//! Many accounts allow only a single connection, so every path here
//! must tear down before it opens. Never hold two streams, even briefly.
//! `loadfile ... replace` satisfies that by construction: mpv closes the
//! current demuxer before opening the next.
//!
//! **Threading.** Commands arrive on Tauri's async worker threads. The player
//! and the video surface live in main-thread locals (the surface's toolkit
//! objects are `!Send` on every platform), so every command that touches them
//! hops with [`tauri::AppHandle::run_on_main_thread`] and carries plain data
//! across - never a toolkit handle. The shared state that
//! both sides read is a `Mutex<Machine>` here, which is ordinary `Send` data.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};

use crate::catalog::activity::{self, Accumulator};
use crate::catalog::store;
use crate::ipc::types::{events, PlaybackState, PlayerStateDto};
use crate::surface::mpv::{self, Player, Probe, Signal};
use crate::surface::VideoRect;
use crate::source::{Playable, Source};
use crate::xtream::XtreamClient;

/// How long a stream gets to produce its first frame before it is called dead.
/// Matches mpv's own `network-timeout`, set in [`Player::new`].
const FIRST_FRAME_DEADLINE: Duration = Duration::from_secs(8);

/// State-feed heartbeat. Fast enough that a channel change feels instant,
/// slow enough that it costs nothing.
const TICK: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------------------
// Main-thread access
// ---------------------------------------------------------------------------

/// Runs `f` on the main thread and waits for its result.
///
/// Blocking here is deliberate and bounded: the command is already on a worker
/// thread, and a hop that never answers must fail rather than wedge the
/// frontend forever.
async fn on_main<R: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce() -> Result<R, String> + Send + 'static,
) -> Result<R, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| format!("could not reach the main thread: {e}"))?;
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| "the main thread did not answer".to_owned())?
}

/// Runs `f` against the player on the main thread and waits for its result.
/// Commands arrive on worker threads; the player lives on the main thread.
async fn on_player<R: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce(&Player) -> Result<R, String> + Send + 'static,
) -> Result<R, String> {
    on_main(app, move || {
        // `None`, not a hang: a command that arrives before `setup` has built
        // the player should say so immediately.
        mpv::with_player(f).unwrap_or_else(|| Err("the player is not running".to_owned()))
    })
    .await
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Everything the frontend sees, plus the bookkeeping that decides when it
/// changes. Held behind a `Mutex` so worker threads and the main-thread ticker can
/// both drive it.
struct Machine {
    dto: PlayerStateDto,
    /// When the current open began. The first-frame deadline runs off it.
    opened_at: Option<Instant>,
    /// The last payload actually emitted, so an unchanged tick stays silent.
    last_emitted: Option<String>,
    /// Whether playback should stay at the live edge.
    ///
    /// True from the moment a channel opens: the server bursts on connect and
    /// mpv starts at the oldest cached frame, which lands playback ~20s in the
    /// past, and the burst keeps the edge running away for a while afterwards.
    /// A single catch-up at first frame is not enough. Cleared when the viewer
    /// deliberately rewinds - at that point being behind is the point - and
    /// set again by `seek_live`.
    following_live: bool,
    /// Playback time genuinely lost to stalls since the last catch-up.
    /// See [`lost_since`]: this, not the depth of the buffer, is what means
    /// the viewer has fallen behind.
    drift_secs: f64,
    /// Where the playhead was last tick, to measure that against.
    last_pos: Option<f64>,
    /// When that reading was taken, so drift is measured against the wall
    /// clock rather than against the heartbeat's nominal interval.
    last_tick_at: Option<Instant>,
    /// When the last automatic catch-up ran, so chasing cannot thrash.
    last_live_nudge: Option<Instant>,
}

impl Machine {
    fn new() -> Self {
        Self {
            dto: PlayerStateDto::default(),
            opened_at: None,
            last_emitted: None,
            following_live: false,
            drift_secs: 0.0,
            last_pos: None,
            last_tick_at: None,
            last_live_nudge: None,
        }
    }

    fn opening(&mut self, stream_id: i64) {
        self.dto.at_live_edge = true;
        self.following_live = true;
        self.last_live_nudge = None;
        self.drift_secs = 0.0;
        self.last_pos = None;
        self.last_tick_at = None;
        self.dto.state = PlaybackState::Opening;
        self.dto.stream_id = Some(stream_id);
        self.dto.error = None;
        // The previous stream's measurements are not this stream's. Clearing
        // them is what makes the quality readout honest across a zap.
        self.dto.width = None;
        self.dto.height = None;
        self.dto.fps = None;
        self.dto.codec = None;
        self.dto.behind_live = 0.0;
        self.dto.buffered_back = 0.0;
        self.opened_at = Some(Instant::now());
    }

    /// First frame decoded. The only honest "it works" edge: `file-loaded`
    /// fires on a stream that connects and then stalls.
    fn playing(&mut self) {
        if matches!(self.dto.state, PlaybackState::Stopped) {
            return;
        }
        self.dto.state = if self.dto.paused_flag() {
            PlaybackState::Paused
        } else {
            PlaybackState::Playing
        };
        self.dto.error = None;
        self.opened_at = None;
    }

    fn set_paused(&mut self, paused: bool) {
        match self.dto.state {
            PlaybackState::Playing if paused => self.dto.state = PlaybackState::Paused,
            PlaybackState::Paused if !paused => self.dto.state = PlaybackState::Playing,
            _ => {}
        }
    }

    fn failed(&mut self, why: impl Into<String>) {
        self.following_live = false;
        self.dto.state = PlaybackState::Failed;
        self.dto.error = Some(why.into());
        self.opened_at = None;
    }

    fn stopped(&mut self) {
        self.dto.at_live_edge = true;
        self.following_live = false;
        self.dto.state = PlaybackState::Stopped;
        self.dto.stream_id = None;
        self.dto.error = None;
        self.dto.behind_live = 0.0;
        self.dto.buffered_back = 0.0;
        self.opened_at = None;
    }

    /// mpv ended the file on its own. Before a first frame that is a failure,
    /// not a tidy stop.
    fn ended(&mut self) {
        self.dto.at_live_edge = true;
        self.following_live = false;
        match self.dto.state {
            PlaybackState::Opening => self.failed("the stream ended before a frame arrived"),
            PlaybackState::Playing | PlaybackState::Paused => self.stopped(),
            _ => {}
        }
    }

    /// Folds a main-thread measurement in. Quality figures are only trusted
    /// once something is actually on screen.
    fn apply(&mut self, p: &Probe) {
        if matches!(
            self.dto.state,
            PlaybackState::Playing | PlaybackState::Paused
        ) {
            if p.width.is_some() {
                self.dto.width = p.width;
            }
            if p.height.is_some() {
                self.dto.height = p.height;
            }
            if p.fps.is_some() {
                self.dto.fps = p.fps;
            }
            if p.codec.is_some() {
                self.dto.codec = p.codec.clone();
            }
            self.dto.behind_live = round_tenth(p.behind_live);
            self.dto.at_live_edge = live_edge_with_hysteresis(
                self.dto.at_live_edge,
                self.dto.behind_live,
            );
            self.dto.buffered_back = round_tenth(p.buffered_back);
            self.set_paused(p.paused);
        }
    }

    /// A stream that connects and never decodes must not sit on `Opening`
    /// forever; that is the failure the user actually hits on a dead channel.
    fn check_deadline(&mut self) {
        if matches!(self.dto.state, PlaybackState::Opening)
            && self
                .opened_at
                .is_some_and(|t| t.elapsed() >= FIRST_FRAME_DEADLINE)
        {
            self.failed("no video within 8s");
        }
    }

    /// The payload to emit, or `None` if nothing the frontend can see moved.
    fn changed(&mut self) -> Option<PlayerStateDto> {
        let now = serde_json::to_string(&self.dto).ok()?;
        if self.last_emitted.as_deref() == Some(now.as_str()) {
            return None;
        }
        self.last_emitted = Some(now);
        Some(self.dto.clone())
    }
}

impl PlayerStateDto {
    fn paused_flag(&self) -> bool {
        matches!(self.state, PlaybackState::Paused)
    }
}

/// Whole tenths of a second. Raw cache timings jitter in the third decimal,
/// and a state event per jitter is a re-render per jitter.
fn round_tenth(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

static STATE: LazyLock<Mutex<Machine>> = LazyLock::new(|| Mutex::new(Machine::new()));

/// Watch time earned by whatever is on, for auto-favourites.
///
/// Driven from the heartbeat below, which is the only place that knows the
/// difference between a stream that is playing and one that is merely open.
/// The policy — what counts, and when it is worth a write — is
/// [`crate::catalog::activity::Accumulator`], and is pure.
static WATCHED: LazyLock<Mutex<Accumulator>> = LazyLock::new(|| Mutex::new(Accumulator::new()));

/// Writes earned watch time, off the main thread.
///
/// Never inline: this runs from the heartbeat, and the shared connection can
/// be held by a catalogue read for as long as that read takes. Blocking the
/// main thread on it would stall the frame the compositor is waiting for. A
/// failure is logged and dropped — losing thirty seconds of bookkeeping is not
/// worth interrupting playback over.
fn persist(earned: activity::Earned) {
    tauri::async_runtime::spawn_blocking(move || {
        let r = crate::catalog::db::with_db(|c| {
            store::record_activity(c, earned.stream_id, earned.seconds, earned.open, earned.at)
        });
        if let Err(e) = r {
            tracing::warn!("could not record watch time: {e}");
        }
    });
}

/// Mutates the state and emits `PLAYER_STATE` if anything visible changed.
fn commit(app: &AppHandle, f: impl FnOnce(&mut Machine)) {
    let payload = {
        let mut m = STATE.lock().expect("player state mutex poisoned");
        f(&mut m);
        m.changed()
    };
    if let Some(p) = payload {
        let _ = app.emit(events::PLAYER_STATE, p);
    }
}

// ---------------------------------------------------------------------------
// The heartbeat
// ---------------------------------------------------------------------------

static TICKER_STARTED: AtomicBool = AtomicBool::new(false);

/// Starts the main-thread heartbeat once, on the first play.
///
/// It is what turns mpv's event queue into state transitions, and the queue
/// has to be drained by somebody or it overflows.
fn ensure_ticker(app: &AppHandle) {
    if TICKER_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let app = app.clone();
    if let Err(e) = app.clone().run_on_main_thread(move || {
        mpv::start_ticker(TICK, move |p| tick(&app, p));
    }) {
        // Nothing else drives the state feed, so a failure here is not
        // cosmetic; let the next play try again.
        TICKER_STARTED.store(false, Ordering::Release);
        tracing::error!("could not start the player state ticker: {e}");
    }
}

/// One heartbeat, on the main thread.
fn tick(app: &AppHandle, p: &Player) {
    let signals = p.pump();
    let probe = p.probe();
    // The diagnostics graphs are drawn from a record kept whether or not the
    // panel is open, so that opening it shows the minute that has just
    // happened rather than an empty box. Four property reads, on a tick that
    // was going to run anyway.
    let health = p.health();
    // Assigned inside the block below, where the state lock is held.
    let earned;
    let loaded;
    let payload = {
        let mut m = STATE.lock().expect("player state mutex poisoned");
        for s in &signals {
            match s {
                Signal::VideoReconfig => m.playing(),
                Signal::EndFile { error: Some(e), .. } => m.failed(e.clone()),
                // A file ended because we replaced it is the *previous*
                // stream's obituary arriving after the new one opened.
                // Acting on it would knock a healthy zap back to Stopped.
                Signal::EndFile {
                    error: None,
                    replaced,
                } => {
                    if !replaced {
                        m.ended();
                    }
                }
                Signal::StartFile | Signal::FileLoaded => {}
            }
        }
        m.apply(&probe);
        m.check_deadline();

        // Catch up only after playback has genuinely lost time.
        //
        // Not by chasing `behind_live`, which is not what it sounds like: it
        // is the distance to the end of the *readahead buffer*, not to
        // anything happening now. A server sends a burst when a channel
        // opens, so the buffer is immediately many seconds deep while the
        // playhead is doing nothing wrong — measured against a server
        // delivering at 4x, it crossed a six second threshold 3.1 seconds in
        // and peaked past 20. A rule on that depth seeks, waits three
        // seconds, and seeks again, for as long as the burst lasts: a
        // stutter-jump-stutter at the start of every channel change.
        //
        // Lost time cannot be produced by a deep buffer. It only accumulates
        // when the picture actually stops, which is the one case where
        // catching up is what the viewer wants.
        if m.following_live && m.dto.state == PlaybackState::Playing {
            let elapsed = m
                .last_tick_at
                .map_or(TICK.as_secs_f64(), |t| t.elapsed().as_secs_f64());
            // Clamped as a total, not per tick: see `lost_since`. Time made
            // back has to be able to cancel time lost, or timer jitter alone
            // reaches the threshold.
            m.drift_secs =
                (m.drift_secs + lost_since(m.last_pos, probe.time_pos, elapsed)).max(0.0);
            let settled = m
                .last_live_nudge
                .is_none_or(|t| t.elapsed() >= LIVE_NUDGE_INTERVAL);
            if m.drift_secs > MAX_DRIFT_SECS && settled && probe.behind_live > LIVE_EDGE_MARGIN {
                m.last_live_nudge = Some(Instant::now());
                m.drift_secs = 0.0;
                let by = (probe.behind_live - LIVE_EDGE_MARGIN).max(0.0);
                // Best-effort: failing to catch up is not a reason to fail a
                // stream that is otherwise playing fine.
                let _ = p.command("seek", &[&format!("{by:.3}"), "relative"]);
            }
        } else {
            m.drift_secs = 0.0;
        }
        m.last_pos = probe.time_pos;
        m.last_tick_at = Some(Instant::now());

        // Watch time, for auto-favourites. Only `Playing` counts: opening is
        // not watching, and neither is a paused stream sitting on a frame.
        earned = WATCHED.lock().expect("activity mutex poisoned").observe(
            m.dto.stream_id,
            m.dto.state == PlaybackState::Playing,
            TICK.as_secs_f64(),
            activity::now_secs(),
        );

        // Nothing loaded is not a reading of zero buffer, it is no reading:
        // recording it would fill the graph with a flat line drawn while
        // there was nothing to draw.
        loaded = m.dto.stream_id.is_some();

        m.changed()
    };
    if loaded {
        crate::ipc::health::observe(health);
    }
    if let Some(e) = earned {
        persist(e);
    }
    if let Some(payload) = payload {
        let _ = app.emit(events::PLAYER_STATE, payload);
    }
}

// ---------------------------------------------------------------------------
// Credentials and URLs
// ---------------------------------------------------------------------------

/// Development override pointing playback at a locally served fixture.
///
/// Many accounts allow only a single connection and real streams are
/// non-deterministic, so tests replay a recording instead (see
/// `tools/fake_stream.py`). Set `ZAPPER_FIXTURE_BASE` to that server's root;
/// unset - as in any real run - this returns `None` and the server is used.
fn fixture_url(stream_id: i64) -> Option<String> {
    let base = std::env::var("ZAPPER_FIXTURE_BASE").ok()?;
    let base = base.trim_end_matches('/');
    // A specific fixture may be pinned; otherwise the id names the file, so
    // different ids exercise different formats.
    match std::env::var("ZAPPER_FIXTURE_NAME") {
        Ok(name) => Some(format!("{base}/{name}")),
        Err(_) => Some(format!("{base}/{stream_id}.ts")),
    }
}

/// The process-wide source — Xtream client or playlist — cached but
/// replaceable.
///
/// A `OnceLock` would be simpler, but the account is editable: saving new
/// credentials has to take effect without a restart, and a lock that can
/// only be written once cannot do that.
static CLIENT: std::sync::RwLock<Option<&'static Source>> = std::sync::RwLock::new(None);

/// Drops the cached client so the next call builds one from whatever is on
/// file now. Called when the account changes.
pub fn forget_panel() {
    if let Ok(mut slot) = CLIENT.write() {
        *slot = None;
    }
}

/// The Xtream client. Built on demand; a failure is not cached, so fixing the
/// account does not need a restart.
///
/// Public so the verification harness can resolve a channel to play without
/// a second copy of the credential loader - and without a stream URL ever
/// reaching a command line, where it would be world-readable through `/proc`.
pub fn panel() -> Result<&'static XtreamClient, String> {
    match source()? {
        Source::Xtream(c) => Ok(c),
        Source::M3u(_) => Err("the account is an M3U playlist, not an Xtream panel".to_owned()),
    }
}

/// The account in use, of whichever kind, built on demand like [`xtream()`].
///
/// Errors are mapped to plain strings that reach the UI, so they are
/// scrubbed: a playlist URL can carry the account.
pub fn source() -> Result<&'static Source, String> {
    if let Ok(slot) = CLIENT.read() {
        if let Some(c) = *slot {
            return Ok(c);
        }
    }
    let built = Source::load().map_err(|e| crate::catalog::sync_engine::scrub(&e.to_string()))?;
    // Leaked on purpose: the client lives for the process, and the few bytes
    // a replaced one costs are not worth threading a lifetime through every
    // caller. An account can only be changed a handful of times by hand.
    let leaked: &'static Source = Box::leak(Box::new(built));
    if let Ok(mut slot) = CLIENT.write() {
        *slot = Some(leaked);
    }
    Ok(leaked)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn play(app: AppHandle, stream_id: i64) -> Result<(), String> {
    let target = match fixture_url(stream_id) {
        Some(u) => Playable::plain(u),
        None => source()?
            .live(stream_id)
            .map_err(|e| crate::catalog::sync_engine::scrub(&e.to_string()))?,
    };
    open(&app, stream_id, target).await
}

/// Replays a past programme on an archive-enabled channel.
/// `start` is Unix seconds; `duration_minutes` bounds the request.
#[tauri::command]
pub async fn play_catchup(
    app: AppHandle,
    stream_id: i64,
    start: i64,
    duration_minutes: u32,
) -> Result<(), String> {
    let target = match source()? {
        Source::Xtream(panel) => {
            // The panel reads the timestamp in its own wall clock, which is the
            // machine's: sending UTC replays the wrong hour for most of the world.
            let stamp = timeshift_stamp(start, local_utc_offset(start));
            Playable::plain(panel.timeshift_url(stream_id, &stamp, duration_minutes))
        }
        // A playlist's archive is whatever its entry's template says; the
        // local offset is there for templates written in wall-clock time.
        Source::M3u(m) => m
            .catchup(
                stream_id,
                start,
                duration_minutes,
                activity::now_secs(),
                local_utc_offset(start),
            )
            .map_err(|e| crate::catalog::sync_engine::scrub(&e.to_string()))?,
    };
    open(&app, stream_id, target).await
}

/// Whether the last stream opened set its own HTTP headers. See
/// [`apply_http_headers`].
static CUSTOM_HEADERS: AtomicBool = AtomicBool::new(false);

/// Hands a stream's user agent and referrer to mpv before it is opened.
///
/// These are global options in mpv, read when a file opens, so they have to
/// be put back for the next stream that does not set them — a playlist
/// channel's referrer must not follow the viewer to the next channel. An
/// Xtream channel never sets either, and while nothing has set them there is
/// nothing to undo, so on an Xtream account this makes no mpv calls at all.
///
/// Failures are logged rather than returned: a stream that wanted a header
/// may still play without one, and refusing to try would be worse.
fn apply_http_headers(p: &Player, target: &Playable) {
    let custom = target.user_agent.is_some() || target.referrer.is_some();
    if !custom && !CUSTOM_HEADERS.load(Ordering::Acquire) {
        return;
    }
    let ua = target
        .user_agent
        .as_deref()
        .unwrap_or(crate::m3u::fetch::DEFAULT_USER_AGENT);
    let referrer = target.referrer.as_deref().unwrap_or("");
    for (key, value) in [("user-agent", ua), ("referrer", referrer)] {
        if let Err(e) = p.set_property(key, value) {
            tracing::warn!("could not set {key} for this stream: {e}");
        }
    }
    CUSTOM_HEADERS.store(custom, Ordering::Release);
}

/// Shared by live and catchup: state first, then the one connection.
async fn open(app: &AppHandle, stream_id: i64, target: Playable) -> Result<(), String> {
    ensure_ticker(app);
    // The scrollback previews describe a buffer that is about to be closed.
    // Keeping them would put the previous channel's picture under the cursor.
    crate::surface::trickplay::clear();
    // Same reasoning, for the same reason: the graphs describe one stream.
    crate::ipc::health::clear();
    commit(app, |m| m.opening(stream_id));
    let r = on_player(app, move |p| {
        apply_http_headers(p, &target);
        // `replace` closes the current demuxer before opening the next, which
        // is what keeps this a one-connection client.
        p.load(&target.url).map_err(|e| e.to_string())
    })
    .await;
    if let Err(e) = &r {
        commit(app, |m| m.failed(e.clone()));
    }
    r
}

#[tauri::command]
pub async fn stop(app: AppHandle) -> Result<(), String> {
    crate::surface::trickplay::clear();
    let r = on_player(&app, |p| p.command("stop", &[]).map_err(|e| e.to_string())).await;
    commit(&app, |m| m.stopped());
    r
}

#[tauri::command]
pub async fn toggle_pause(app: AppHandle) -> Result<(), String> {
    let paused = on_player(&app, |p| {
        let want = !p.get_bool("pause").unwrap_or(false);
        p.set_flag("pause", want).map_err(|e| e.to_string())?;
        Ok(want)
    })
    .await?;
    commit(&app, |m| m.set_paused(paused));
    Ok(())
}

/// Negative seeks back. Clamped to the buffered window.
#[tauri::command]
pub async fn seek_relative(app: AppHandle, seconds: f64) -> Result<(), String> {
    // Rewinding is a deliberate choice to be behind, so stop chasing the edge.
    // Seeking forward is the viewer catching up themselves and leaves the mode
    // alone; the automatic follow will finish the job if they stop short.
    if seconds < 0.0 {
        STATE
            .lock()
            .expect("player state mutex poisoned")
            .following_live = false;
    }
    on_player(&app, move |p| {
        let probe = p.probe();
        let by = clamp_seek(seconds, probe.buffered_back, probe.behind_live);
        if by.abs() < 0.05 {
            return Ok(());
        }
        p.command("seek", &[&format!("{by:.3}"), "relative"])
            .map_err(|e| e.to_string())
    })
    .await
}

/// How far short of the seekable end `seek_live` aims.
///
/// Small but not zero: the boundary itself moves between the read of
/// `seekable-ranges` and the seek, and a target one frame past it is refused
/// silently rather than clamped.
const LIVE_EDGE_MARGIN: f64 = 0.5;

/// How much playback time may be lost before the viewer is carried back to
/// the live edge.
///
/// Ten seconds is a long stall — long enough that it was a real
/// interruption and not a dropped frame, and short enough that a viewer who
/// sat through it has not silently become a viewer of the past.
const MAX_DRIFT_SECS: f64 = 10.0;

/// How far playback fell behind the clock since the previous tick.
///
/// Positive when the picture lost time, negative when it made some back, and
/// the sign matters: this is summed, and a running total that could only
/// ever grow would ratchet. The heartbeat is a timer, so a tick is never
/// exactly its nominal length and the difference lands here as a few
/// milliseconds either way. Keeping only the losses would turn that jitter
/// into ten seconds of "drift" every few minutes and carry the viewer to the
/// live edge for no reason at all. The caller clamps the *total* at zero, so
/// healthy playback hovers there instead of climbing.
///
/// Measured against the wall clock rather than the nominal interval, because
/// a late tick is the timer's fault and not the stream's.
///
/// Zero when the position moved backwards or leapt forwards: those are
/// seeks, and charging a seek as drift would have a rewind immediately drag
/// the viewer back to live, the precise opposite of what they just asked
/// for. The forward bound is generous because a tick can run long.
fn lost_since(previous: Option<f64>, now: Option<f64>, elapsed: f64) -> f64 {
    let (Some(before), Some(after)) = (previous, now) else {
        return 0.0;
    };
    if !elapsed.is_finite() || elapsed <= 0.0 {
        return 0.0;
    }
    let advanced = after - before;
    if advanced < 0.0 || advanced > elapsed * 4.0 {
        return 0.0;
    }
    elapsed - advanced
}

/// Below this, playback is considered to have reached live.
const LIVE_ENTER: f64 = 3.0;
/// Above this, it is considered to have fallen behind.
///
/// The gap between the two is the point. `behind_live` is measured against the
/// end of mpv's seekable window, which advances in chunks as the demuxer
/// reads, so it sawtooths by a couple of seconds while playback is perfectly
/// live. A single threshold makes the indicator flicker between "live" and a
/// few seconds behind; two thresholds mean it only changes when something
/// actually changed.
const LIVE_EXIT: f64 = 8.0;

/// Deliberately takes the previous answer: without it there is no hysteresis,
/// only a threshold.
fn live_edge_with_hysteresis(was_live: bool, behind: f64) -> bool {
    if was_live {
        behind <= LIVE_EXIT
    } else {
        behind <= LIVE_ENTER
    }
}

/// Minimum gap between automatic catch-ups, so a stream that cannot keep up
/// is not seeked on every tick.
const LIVE_NUDGE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);

#[tauri::command]
pub async fn seek_live(app: AppHandle) -> Result<(), String> {
    // Asking for live means staying there, not visiting once.
    {
        let mut m = STATE.lock().expect("player state mutex poisoned");
        m.following_live = true;
        m.last_live_nudge = None;
    }
    on_player(&app, |p| {
        let probe = p.probe();
        // Relative, the same operation the backward seek already does, in
        // reverse - and bounded by `behind_live`, which is measured to the end
        // of mpv's *seekable* window rather than to the end of its cache. A
        // seek past the seekable end (the cache runs one to three seconds
        // beyond it) is ignored by mpv without an error: playback stays
        // exactly where it was.
        let by = (probe.behind_live - LIVE_EDGE_MARGIN).max(0.0);
        match by > 0.05 {
            true => p.command("seek", &[&format!("{by:.3}"), "relative"]),
            false => Ok(()),
        }
        .map_err(|e| e.to_string())
    })
    .await
}

/// 0-200; above 100 is software boost via `volume-max`.
#[tauri::command]
pub async fn set_volume(app: AppHandle, volume: f64) -> Result<(), String> {
    let v = volume.clamp(0.0, 200.0);
    on_player(&app, move |p| {
        p.set_f64("volume", v).map_err(|e| e.to_string())
    })
    .await?;
    commit(&app, |m| m.dto.volume = v);
    Ok(())
}

#[tauri::command]
pub async fn set_muted(app: AppHandle, muted: bool) -> Result<(), String> {
    on_player(&app, move |p| {
        p.set_flag("mute", muted).map_err(|e| e.to_string())
    })
    .await?;
    commit(&app, |m| m.dto.muted = muted);
    Ok(())
}

/// Toggles loudness normalisation. Named "compressor" because that is what
/// the control is usually called in players, but see [`compressor_filter`]
/// for why it is not a classic compressor.
#[tauri::command]
pub async fn set_compressor(app: AppHandle, enabled: bool) -> Result<(), String> {
    let filter = if enabled {
        compressor_filter()
    } else {
        String::new()
    };
    on_player(&app, move |p| {
        p.set_property("af", &filter).map_err(|e| e.to_string())
    })
    .await?;
    commit(&app, |m| m.dto.compressor = enabled);
    Ok(())
}

/// Video is not a DOM element; the frontend sends where it should be drawn.
#[tauri::command]
pub async fn set_video_rect(app: AppHandle, rect: VideoRect) -> Result<(), String> {
    on_main(&app, move || crate::surface::set_video_rect(rect)).await
}

/// The scrollback preview nearest a point in the buffer.
///
/// `behind_live` is the same number the scrubber speaks in everywhere else,
/// so the interface does not have to know that the thumbnails are keyed by
/// stream time. `None` is an ordinary answer: nothing has been captured near
/// that moment yet, or the archive was asked about, which is left of the
/// buffer and was never on this screen to be photographed.
#[tauri::command]
pub async fn thumbnail_at(
    app: AppHandle,
    behind_live: f64,
) -> Result<Option<crate::ipc::types::ThumbnailDto>, String> {
    if !behind_live.is_finite() || behind_live < 0.0 {
        return Ok(None);
    }
    on_player(&app, move |p| {
        let probe = p.probe();
        // The seekable end is the live edge in stream time, and it is what
        // the thumbnails were stamped against. Falling back to the playhead
        // plus its own lag covers the moments before mpv reports a window.
        let Some(edge) = probe
            .live_edge
            .or_else(|| probe.time_pos.map(|t| t + probe.behind_live))
        else {
            return Ok(None);
        };
        Ok(
            crate::surface::trickplay::nearest_png(edge - behind_live).map(|(at, png)| {
                crate::ipc::types::ThumbnailDto {
                    data_url: format!(
                        "data:image/png;base64,{}",
                        crate::surface::trickplay::base64(&png)
                    ),
                    behind_live: (edge - at).max(0.0),
                }
            }),
        )
    })
    .await
}

/// Everything the diagnostics panel shows: one reading, and the history.
///
/// Polled rather than pushed: it is far more than the state feed needs, and
/// nothing should pay for it while the panel is closed. The history is the
/// exception and is collected continuously — see `ipc::health` for why, and
/// for how little it costs — but it still only crosses the boundary when
/// somebody asks, which is what makes one call enough to draw the panel.
#[tauri::command]
pub async fn player_stats(app: AppHandle) -> Result<crate::ipc::types::StatsDto, String> {
    on_player(&app, |p| {
        let s = p.stats();
        let probe = p.probe();
        Ok(crate::ipc::types::StatsDto {
            container: s.container,
            video_codec: s.video_codec,
            audio_codec: s.audio_codec,
            hwdec: s.hwdec,
            pixel_format: s.pixel_format,
            primaries: s.primaries,
            gamma: s.gamma,
            video_bitrate: s.video_bitrate,
            audio_bitrate: s.audio_bitrate,
            measured_fps: s.measured_fps,
            container_fps: s.container_fps,
            audio_samplerate: s.audio_samplerate,
            audio_channels: s.audio_channels,
            cache_duration: s.cache_duration,
            cache_speed: s.cache_speed,
            demuxer_bytes: s.demuxer_bytes,
            buffering: s.buffering,
            cache_buffering_percent: s.cache_buffering_percent,
            dropped_frames: s.dropped_frames,
            decoder_dropped_frames: s.decoder_dropped_frames,
            avsync: s.avsync,
            behind_live: probe.behind_live,
            buffered_back: probe.buffered_back,
            history: crate::ipc::health::samples(),
        })
    })
    .await
}

#[tauri::command]
pub async fn player_state() -> Result<PlayerStateDto, String> {
    Ok(STATE
        .lock()
        .expect("player state mutex poisoned")
        .dto
        .clone())
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// Clamps a relative seek to what is actually buffered.
///
/// Seeking past either end of the demuxer cache on a live stream drops the
/// connection and reopens it, which this account cannot afford.
fn clamp_seek(seconds: f64, buffered_back: f64, behind_live: f64) -> f64 {
    if seconds < 0.0 {
        seconds.max(-buffered_back)
    } else {
        seconds.min(behind_live)
    }
}

/// Peak ceiling for the limiter, as a linear amplitude. -1 dBFS.
///
/// The headroom is deliberate: a lossy decoder reconstructs inter-sample
/// peaks above the highest stored sample, so normalising to 0 clips on the
/// way out.
const LIMIT_AMPLITUDE: f64 = 0.891;

/// `speechnorm` expansion factor. How much gain a quiet passage may be
/// given; 6.25 lifts broadcast levels to desktop levels without pumping.
const NORM_EXPANSION: f64 = 6.25;

/// How fast the gain is allowed to rise between samples. Small, so the lift
/// arrives over a moment rather than on a syllable.
const NORM_RAISE: f64 = 0.0001;

/// mpv's `af` value for the compressor toggle.
///
/// **Not `acompressor`, and not `loudnorm`.** Both were tried and both were
/// wrong, in ways worth recording so neither comes back.
///
/// `acompressor` was tried with a commonly shared set of web-audio
/// `DynamicsCompressorNode` settings. Those describe a compressor in front
/// of a gain stage; alone they only attenuate, and ffmpeg applies no makeup
/// unless asked. It made everything about 20 dB quieter.
///
/// `loudnorm` fixed the loudness and broke the control. It only accepts
/// 192 kHz, so inserting it made mpv reopen the audio device at 192 kHz and
/// close it again on the way out — and the first time, that cost **3.8
/// seconds of frozen picture**. Measured with `cargo run --bin filter_bench`:
///
/// ```text
///                        1st toggle   later toggles
///   loudnorm               3790 ms      186-243 ms
///   dynaudnorm + limiter   1918 ms       21-132 ms
///   speechnorm + limiter     33 ms          30 ms
/// ```
///
/// Priming does not help: the cost is paid per playback session, not per
/// process, so warming the filter at startup — over silence or over the
/// stream — leaves the next channel to pay it again.
///
/// `speechnorm` gives up almost nothing for that. Integrated loudness over
/// three test streams (sports, speech, and a synthetic one), against
/// `loudnorm`'s:
///
/// ```text
///                  bypass    speechnorm      loudnorm
///   sport720p60    -25.4     -13.9  -1.0     -14.1  -1.5 dBTP
///   speech         -23.3     -13.9  -1.0     -14.1  -1.5
///   motion1080p     -6.7      -7.3  -0.2     -14.0  -4.9
/// ```
///
/// Identical on the two that look like real broadcast audio. They differ
/// only on the synthetic one that is already hot, which `speechnorm` leaves
/// alone rather than pulling down 7 dB — and which it still rescues from
/// clipping, +2.4 to -0.2 dBTP. Leaving loud things loud is also the point:
/// the control is there to lift quiet audio.
///
/// -14 LUFS, where both land, is what YouTube and Spotify normalise to, so
/// this sits at the same level as everything else on the desktop.
fn compressor_filter() -> String {
    format!(
        "speechnorm=e={NORM_EXPANSION}:r={NORM_RAISE}:l=1,\
         alimiter=limit={LIMIT_AMPLITUDE}:level=disabled"
    )
}

/// Formats Unix seconds as the server's `YYYY-MM-DD:HH-MM`, `offset` seconds
/// east of UTC. Pure, so the calendar arithmetic can be tested without a
/// clock or a time zone.
fn timeshift_stamp(unix: i64, offset: i64) -> String {
    let t = unix + offset;
    let (y, m, d) = civil_from_days(t.div_euclid(86_400));
    let s = t.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}:{:02}-{:02}",
        s / 3600,
        (s % 3600) / 60
    )
}

/// Days since the Unix epoch to a civil date. Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Seconds east of UTC at `unix`, from the C library.
///
/// std has no local time and the dependency set is pinned, so `localtime_r`
/// is resolved out of the already-loaded libc - the same trick the Linux
/// surface uses for `setlocale`. Only the offset is taken; the calendar
/// arithmetic stays in Rust where it can be tested.
#[cfg(unix)]
fn local_utc_offset(unix: i64) -> i64 {
    use libloading::os::unix::Library;
    use std::ffi::{c_char, c_int, c_long};

    /// glibc's `struct tm`: nine ints, then `tm_gmtoff` and `tm_zone`. The
    /// BSDs, macOS among them, lay it out the same way.
    #[repr(C)]
    struct Tm {
        sec: c_int,
        min: c_int,
        hour: c_int,
        mday: c_int,
        mon: c_int,
        year: c_int,
        wday: c_int,
        yday: c_int,
        isdst: c_int,
        gmtoff: c_long,
        zone: *const c_char,
    }

    type LocaltimeR = unsafe extern "C" fn(*const i64, *mut Tm) -> *mut Tm;
    let this = Library::this();
    let Ok(localtime_r) = (unsafe { this.get::<LocaltimeR>(b"localtime_r\0") }) else {
        tracing::warn!("localtime_r did not resolve; catchup times will be sent as UTC");
        return 0;
    };
    let mut tm: Tm = unsafe { std::mem::zeroed() };
    let t = unix;
    if unsafe { localtime_r(&t, &mut tm) }.is_null() {
        return 0;
    }
    tm.gmtoff as i64
}

/// Seconds east of UTC at `unix`, from Windows' own time-zone rules.
///
/// There is no `tm_gmtoff` here. Instead the instant is converted to local
/// time with `SystemTimeToTzSpecificLocalTime`, which applies the daylight
/// rule in force *at that instant* rather than now (a catch-up programme from
/// before the clocks changed must be asked for in the offset it aired in),
/// and the two wall-clock readings are subtracted.
#[cfg(windows)]
fn local_utc_offset(unix: i64) -> i64 {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{
        FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
    };

    /// 100ns ticks between 1601-01-01 (FILETIME's epoch) and 1970-01-01.
    const EPOCH_DELTA: i64 = 116_444_736_000_000_000;
    const TICKS_PER_SEC: i64 = 10_000_000;

    let ticks = unix.saturating_mul(TICKS_PER_SEC).saturating_add(EPOCH_DELTA);
    let utc_ft = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    let mut local_ft = FILETIME::default();
    let converted = unsafe {
        FileTimeToSystemTime(&utc_ft, &mut utc).is_ok()
            && SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_ok()
            && SystemTimeToFileTime(&local, &mut local_ft).is_ok()
    };
    if !converted {
        tracing::warn!("could not read the local time zone; catchup times will be sent as UTC");
        return 0;
    }
    let local_ticks =
        (i64::from(local_ft.dwHighDateTime) << 32) | i64::from(local_ft.dwLowDateTime);
    (local_ticks - ticks) / TICKS_PER_SEC
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::{SettingsDto, SubtitleBackdrop, SubtitleStyleDto};

    #[test]
    fn state_walks_stopped_to_opening_to_playing() {
        let mut m = Machine::new();
        assert_eq!(m.dto.state, PlaybackState::Stopped);

        m.opening(498_338);
        assert_eq!(m.dto.state, PlaybackState::Opening);
        assert_eq!(m.dto.stream_id, Some(498_338));

        m.playing();
        assert_eq!(m.dto.state, PlaybackState::Playing);
        assert!(m.dto.error.is_none());
        assert!(m.opened_at.is_none(), "the deadline must be disarmed");
    }

    #[test]
    fn opening_fails_with_a_message() {
        let mut m = Machine::new();
        m.opening(1);
        m.failed("connection refused");
        assert_eq!(m.dto.state, PlaybackState::Failed);
        assert_eq!(m.dto.error.as_deref(), Some("connection refused"));
    }

    #[test]
    fn an_open_that_never_decodes_fails_on_the_deadline() {
        let mut m = Machine::new();
        m.opening(1);
        m.opened_at = Some(Instant::now() - FIRST_FRAME_DEADLINE - Duration::from_secs(1));
        m.check_deadline();
        assert_eq!(m.dto.state, PlaybackState::Failed);
        assert_eq!(m.dto.error.as_deref(), Some("no video within 8s"));
    }

    #[test]
    fn the_deadline_does_not_fire_on_a_playing_stream() {
        let mut m = Machine::new();
        m.opening(1);
        m.playing();
        m.opened_at = Some(Instant::now() - Duration::from_secs(600));
        m.check_deadline();
        assert_eq!(m.dto.state, PlaybackState::Playing);
    }

    #[test]
    fn a_zap_clears_the_previous_streams_measurements() {
        let mut m = Machine::new();
        m.opening(1);
        m.playing();
        m.apply(&Probe {
            width: Some(1920),
            height: Some(1080),
            codec: Some("h264".into()),
            ..Probe::default()
        });
        assert_eq!(m.dto.width, Some(1920));
        m.opening(2);
        assert_eq!(
            m.dto.width, None,
            "channel names lie; stale numbers lie too"
        );
        assert_eq!(m.dto.codec, None);
    }

    #[test]
    fn quality_is_not_taken_before_a_frame_exists() {
        let mut m = Machine::new();
        m.opening(1);
        m.apply(&Probe {
            width: Some(1920),
            ..Probe::default()
        });
        assert_eq!(m.dto.width, None);
    }

    #[test]
    fn an_unchanged_state_emits_once() {
        let mut m = Machine::new();
        assert!(m.changed().is_some(), "the first snapshot is always news");
        assert!(m.changed().is_none());
        m.opening(7);
        assert!(m.changed().is_some());
    }

    #[test]
    fn a_replaced_file_is_not_the_new_streams_end() {
        // Guards the zap race: mpv reports the *previous* file ending after
        // the next one has already been asked for.
        let mut m = Machine::new();
        m.opening(1);
        m.playing();
        m.opening(2);
        m.ended();
        assert_eq!(
            m.dto.state,
            PlaybackState::Failed,
            "a genuine end during Opening is a failure"
        );
    }

    #[test]
    fn seeks_are_clamped_to_the_buffered_window() {
        assert_eq!(clamp_seek(-5.0, 60.0, 3.0), -5.0);
        assert_eq!(clamp_seek(-90.0, 60.0, 3.0), -60.0);
        assert_eq!(clamp_seek(30.0, 60.0, 3.0), 3.0);
        assert_eq!(clamp_seek(-5.0, 0.0, 0.0), 0.0);
    }

    #[test]
    fn a_deep_buffer_is_not_drift() {
        // A server bursts when a channel opens, so the readahead is instantly
        // many seconds deep while playback is perfectly healthy — and a rule
        // that read that depth as "the viewer has fallen behind" would seek,
        // over and over, through the first seconds of every channel. Nothing
        // about buffer depth reaches this function; only the playhead against
        // the clock does.
        assert_eq!(lost_since(Some(10.0), Some(10.25), 0.25), 0.0);
    }

    #[test]
    fn ordinary_timer_jitter_cannot_ratchet_into_a_catch_up() {
        // A tick is never exactly its nominal length, so each one lands a
        // few milliseconds either side. Summed with the losses kept and the
        // gains thrown away, that alone reached ten seconds in a few
        // minutes and yanked a healthy stream to the live edge.
        let mut total: f64 = 0.0;
        let mut pos = 10.0;
        for i in 0..4000 {
            // Alternating slightly short and slightly long ticks.
            let elapsed = if i % 2 == 0 { 0.26 } else { 0.24 };
            pos += 0.25;
            total = (total + lost_since(Some(pos - 0.25), Some(pos), elapsed)).max(0.0);
        }
        // Over a thousand seconds of playback the total stays negligible,
        // and nowhere near the threshold.
        assert!(total < 1.0, "jitter accumulated to {total}");
        assert!(total < MAX_DRIFT_SECS);
    }

    #[test]
    fn a_stall_is_charged_as_drift() {
        let tick = 0.25;
        // Frozen: the whole tick was lost.
        assert_eq!(lost_since(Some(10.0), Some(10.0), tick), 0.25);
        // Crawling: most of it was.
        let lost = lost_since(Some(10.0), Some(10.05), tick);
        assert!((lost - 0.20).abs() < 1e-9, "{lost}");
        // And a tick that ran long is the timer's fault, not the stream's:
        // playback advanced the full wall-clock interval, so nothing is owed.
        assert!(lost_since(Some(10.0), Some(10.40), 0.40).abs() < 1e-9);
        // Forty ticks of frozen picture is the ten seconds that triggers a
        // catch-up, which is the intended order of magnitude.
        assert!((MAX_DRIFT_SECS / 0.25 - 40.0).abs() < 1e-9);
    }

    #[test]
    fn seeks_are_not_charged_as_drift() {
        let tick = 0.25;
        // A rewind. Charging this would drag the viewer straight back to
        // live — the opposite of what they just asked for.
        assert_eq!(lost_since(Some(60.0), Some(55.0), tick), 0.0);
        // A jump forward, including the catch-up seek itself.
        assert_eq!(lost_since(Some(10.0), Some(30.0), tick), 0.0);
        // Nothing to compare against yet, at the first tick of a stream.
        assert_eq!(lost_since(None, Some(10.0), tick), 0.0);
        assert_eq!(lost_since(Some(10.0), None, tick), 0.0);
    }

    #[test]
    fn compressor_lifts_quiet_audio_and_catches_the_peaks() {
        // Pinned in full. A silent edit here changes how loud every channel
        // plays: once it dropped the audio 19 dB, and once it froze the
        // picture for 3.8 seconds on the first toggle. Neither said anything
        // in the logs.
        assert_eq!(
            compressor_filter(),
            "speechnorm=e=6.25:r=0.0001:l=1,alimiter=limit=0.891:level=disabled"
        );
    }

    #[test]
    fn the_limiter_leaves_room_for_inter_sample_peaks() {
        // -1 dBFS. A lossy decoder reconstructs peaks above the highest
        // stored sample, so a ceiling of 1.0 clips on the way out.
        assert!(LIMIT_AMPLITUDE < 1.0);
        assert!(LIMIT_AMPLITUDE > 0.8);
        // Enough lift to carry broadcast levels to desktop levels. Zero or
        // one would make the control do nothing at all.
        assert!(NORM_EXPANSION > 1.0);
        // Gain has to arrive over a moment, not on a syllable.
        assert!(NORM_RAISE > 0.0 && NORM_RAISE < 0.01);
    }

    #[test]
    fn timeshift_stamp_is_local_wall_clock() {
        // 2026-09-08T01:30:00Z.
        let t = 1_788_831_000;
        assert_eq!(timeshift_stamp(t, 0), "2026-09-08:01-30");
        // UTC-4: the same instant is the previous evening, and sending the
        // UTC date would replay the wrong programme.
        assert_eq!(timeshift_stamp(t, -4 * 3600), "2026-09-07:21-30");
        // UTC+5:30, to prove the minute field is not just the UTC minute.
        assert_eq!(timeshift_stamp(t, 5 * 3600 + 1800), "2026-09-08:07-00");
    }

    #[test]
    fn timeshift_stamp_handles_the_epoch_and_a_leap_day() {
        assert_eq!(timeshift_stamp(0, 0), "1970-01-01:00-00");
        // 2024-02-29T23:59:00Z.
        assert_eq!(timeshift_stamp(1_709_251_140, 0), "2024-02-29:23-59");
    }

    #[test]
    fn the_local_offset_is_a_real_time_zone() {
        // Whatever zone the machine is in, the platform's answer has to look
        // like one: within UTC-12..UTC+14 and on a quarter hour. A unit or
        // epoch mistake (Windows counts 100ns ticks from 1601) fails this
        // by orders of magnitude.
        for t in [0, 1_709_251_140, 1_788_831_000] {
            let off = local_utc_offset(t);
            assert!((-12 * 3600..=14 * 3600).contains(&off), "offset {off}s at {t}");
            assert_eq!(off % 900, 0, "offset {off}s at {t} is not on a quarter hour");
        }
    }

    // -----------------------------------------------------------------
    // Settings
    // -----------------------------------------------------------------

    fn prop<'a>(props: &'a [(&str, String)], key: &str) -> &'a str {
        props
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
            .unwrap_or_else(|| panic!("no {key} in {props:?}"))
    }

    fn styled(f: impl FnOnce(&mut SubtitleStyleDto)) -> Vec<(&'static str, String)> {
        let mut s = SubtitleStyleDto::default();
        f(&mut s);
        subtitle_properties(&s)
    }

    #[test]
    fn a_colour_carries_its_alpha_byte_first() {
        // The one the settings panel actually ships: white at a quarter.
        // 0.25 x 255 is 63.75, and the byte is 0x40, not 0x3F.
        assert_eq!(mpv_color("#ffffff", 0.25), "#40FFFFFF");
        assert_eq!(mpv_color("#ffffff", 1.0), "#FFFFFFFF");
        assert_eq!(mpv_color("#ffffff", 0.0), "#00FFFFFF");
        assert_eq!(mpv_color("#ffffff", 0.5), "#80FFFFFF");
        // Channel order survives: mpv is RRGGBB after the alpha, so a colour
        // that is not grey would come back reversed if it did not.
        assert_eq!(mpv_color("#123456", 1.0), "#FF123456");
        assert_eq!(mpv_color("#ff0000", 1.0), "#FFFF0000");
    }

    #[test]
    fn a_colour_is_forgiving_about_its_input() {
        assert_eq!(mpv_color("ffffff", 1.0), "#FFFFFFFF");
        assert_eq!(mpv_color("#ABCDEF", 1.0), "#FFABCDEF");
        assert_eq!(mpv_color("  #abcdef  ", 1.0), "#FFABCDEF");
        // CSS shorthand, because a colour input may well hand one over.
        assert_eq!(mpv_color("#f0a", 1.0), "#FFFF00AA");
        // Anything unreadable is white, not a panic and not a black subtitle
        // on a dark scene.
        assert_eq!(mpv_color("rebeccapurple", 1.0), "#FFFFFFFF");
        assert_eq!(mpv_color("", 1.0), "#FFFFFFFF");
        assert_eq!(mpv_color("#12345", 1.0), "#FFFFFFFF");
        assert_eq!(mpv_color("#gggggg", 1.0), "#FFFFFFFF");
    }

    #[test]
    fn an_opacity_outside_the_unit_range_is_clamped() {
        assert_eq!(mpv_color("#ffffff", 4.0), "#FFFFFFFF");
        assert_eq!(mpv_color("#ffffff", -1.0), "#00FFFFFF");
        assert_eq!(mpv_color("#ffffff", f64::NAN), "#FFFFFFFF");
    }

    #[test]
    fn the_font_and_size_reach_mpv_by_their_own_names() {
        let p = styled(|s| {
            s.font = "Inter".into();
            s.size = 52;
            s.color = "#ffee00".into();
        });
        assert_eq!(prop(&p, "sub-font"), "Inter");
        assert_eq!(prop(&p, "sub-font-size"), "52");
        // The text itself is always opaque; the opacity control is the
        // backdrop's, not the letters'.
        assert_eq!(prop(&p, "sub-color"), "#FFFFEE00");
    }

    #[test]
    fn an_empty_font_falls_back_rather_than_unsetting_mpvs() {
        let p = styled(|s| s.font = "   ".into());
        assert_eq!(prop(&p, "sub-font"), "sans-serif");
    }

    #[test]
    fn background_is_a_box_hugging_the_text_at_the_chosen_opacity() {
        let p = styled(|s| {
            s.backdrop = SubtitleBackdrop::Background;
            s.backdrop_opacity = 0.25;
        });
        assert_eq!(prop(&p, "sub-border-style"), "background-box");
        assert_eq!(prop(&p, "sub-back-color"), "#40000000");
    }

    #[test]
    fn window_is_the_opaque_box() {
        let p = styled(|s| {
            s.backdrop = SubtitleBackdrop::Window;
            s.backdrop_opacity = 0.8;
        });
        assert_eq!(prop(&p, "sub-border-style"), "opaque-box");
        assert_eq!(prop(&p, "sub-back-color"), "#CC000000");
    }

    #[test]
    fn outline_draws_a_border_and_no_box() {
        let p = styled(|s| {
            s.backdrop = SubtitleBackdrop::Outline;
            s.outline_size = 2.5;
            s.shadow_offset = 9.0;
        });
        assert_eq!(prop(&p, "sub-border-style"), "outline-and-shadow");
        assert_eq!(prop(&p, "sub-outline-size"), "2.5");
        // The other backdrops' controls are inert here, and must be sent as
        // zero rather than left at whatever the last backdrop set.
        assert_eq!(prop(&p, "sub-shadow-offset"), "0");
        assert_eq!(prop(&p, "sub-back-color"), TRANSPARENT);
    }

    #[test]
    fn shadow_draws_an_offset_and_no_border() {
        let p = styled(|s| {
            s.backdrop = SubtitleBackdrop::Shadow;
            s.outline_size = 9.0;
            s.shadow_offset = 1.5;
        });
        assert_eq!(prop(&p, "sub-border-style"), "outline-and-shadow");
        assert_eq!(prop(&p, "sub-outline-size"), "0");
        assert_eq!(prop(&p, "sub-shadow-offset"), "1.5");
        assert_eq!(prop(&p, "sub-back-color"), TRANSPARENT);
    }

    #[test]
    fn none_draws_the_letters_and_nothing_else() {
        let p = styled(|s| {
            s.backdrop = SubtitleBackdrop::None;
            s.outline_size = 9.0;
            s.shadow_offset = 9.0;
            s.backdrop_opacity = 0.9;
        });
        assert_eq!(prop(&p, "sub-outline-size"), "0");
        assert_eq!(prop(&p, "sub-shadow-offset"), "0");
        assert_eq!(prop(&p, "sub-back-color"), TRANSPARENT);
    }

    #[test]
    fn every_backdrop_sets_the_same_properties() {
        // Switching backdrop must not leave the previous one's box or border
        // behind: mpv keeps whatever was last set.
        let keys = |b| {
            let mut k: Vec<&str> = styled(|s| s.backdrop = b)
                .iter()
                .map(|(k, _)| *k)
                .collect();
            k.sort_unstable();
            k
        };
        let want = keys(SubtitleBackdrop::Background);
        for b in [
            SubtitleBackdrop::Window,
            SubtitleBackdrop::Outline,
            SubtitleBackdrop::Shadow,
            SubtitleBackdrop::None,
        ] {
            assert_eq!(keys(b), want, "{b:?} sets a different set of properties");
        }
    }

    #[test]
    fn the_rewind_budget_is_minutes_at_the_measured_bitrate() {
        // 18.2 Mbps measured on a 1080p59.94 feed: a minute is 136.5 MB.
        assert_eq!(rewind_bytes(1.0), 136_500_000);
        // The default, which lands near the 512 MiB mpv is initialised with.
        assert_eq!(rewind_bytes(4.0), 546_000_000);
        assert_eq!(rewind_bytes(0.5), 68_250_000);
    }

    #[test]
    fn a_rewind_of_zero_is_a_floor_not_a_disabled_cache() {
        // demuxer-max-back-bytes=0 does not mean "no rewind", it means no
        // back buffer at all - and mpv then cannot seek back even the second
        // it already has, which reads as a broken player rather than a
        // setting.
        assert_eq!(rewind_bytes(0.0), MIN_BACK_BYTES);
        assert_eq!(rewind_bytes(-5.0), MIN_BACK_BYTES);
        assert_eq!(rewind_bytes(f64::NAN), MIN_BACK_BYTES);
        assert_eq!(rewind_bytes(0.000_001), MIN_BACK_BYTES);
    }

    #[test]
    fn an_absurd_rewind_is_capped_rather_than_swapping_the_machine() {
        assert_eq!(rewind_bytes(100_000.0), MAX_BACK_BYTES);
        assert_eq!(rewind_bytes(f64::INFINITY), MAX_BACK_BYTES);
    }

    #[test]
    fn apply_saved_settings_keeps_the_signature_lib_rs_wires() {
        // `lib.rs` belongs to the coordinator, so this is the only place the
        // shape of that call can be held still.
        let _: fn(&tauri::AppHandle) = apply_saved_settings;
    }

    #[test]
    fn playback_settings_map_onto_mpvs_own_names() {
        let on = playback_properties(&SettingsDto {
            rewind_minutes: 2.0,
            hardware_decode: true,
            tone_map_hdr: true,
            ..Default::default()
        });
        assert_eq!(prop(&on, "demuxer-max-back-bytes"), "273000000");
        assert_eq!(prop(&on, "hwdec"), "auto-safe");
        assert_eq!(prop(&on, "tone-mapping"), "auto");

        let off = playback_properties(&SettingsDto {
            hardware_decode: false,
            tone_map_hdr: false,
            ..Default::default()
        });
        assert_eq!(prop(&off, "hwdec"), "no");
        // Not "no": tone-mapping is a choice, and clipping is what "off"
        // means for it.
        assert_eq!(prop(&off, "tone-mapping"), "clip");
    }

    #[test]
    fn the_live_indicator_does_not_flicker_on_the_sawtooth() {
        // behind_live oscillating in the 0-4s band is what normal live
        // playback looks like; the answer must not change through it.
        let mut live = true;
        for behind in [0.0, 3.4, 0.2, 4.0, 1.1, 3.9, 0.0] {
            live = live_edge_with_hysteresis(live, behind);
            assert!(live, "went non-live at {behind}s, which is still live");
        }
    }

    #[test]
    fn a_real_rewind_leaves_the_live_edge_and_stays_left() {
        let mut live = true;
        live = live_edge_with_hysteresis(live, 12.0);
        assert!(!live, "12s behind is not live");
        // Coming back must clear the tighter entry threshold, not merely dip
        // under the exit one, or it would flap around 8s.
        live = live_edge_with_hysteresis(live, 6.0);
        assert!(!live, "6s is under the exit threshold but over the entry one");
        live = live_edge_with_hysteresis(live, 1.0);
        assert!(live, "1s behind is live again");
    }
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// mpv's colour syntax is `#AARRGGBB` — alpha first, and `FF` is opaque.
///
/// Kept pure and tested because the alpha byte is exactly the sort of thing
/// that is off by one forever: a quarter is 63.75 of 255, and rounding it the
/// wrong way is invisible on screen and wrong in every stored setting.
/// Anything unreadable becomes white rather than failing, since a subtitle
/// nobody can read is worse than one in the wrong colour.
fn mpv_color(rgb: &str, opacity: f64) -> String {
    let hex = rgb.trim().trim_start_matches('#');
    let rgb = match hex.len() {
        // CSS shorthand: each digit doubles.
        3 => u32::from_str_radix(hex, 16).ok().map(|v| {
            let (r, g, b) = ((v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf);
            (r * 0x11) << 16 | (g * 0x11) << 8 | (b * 0x11)
        }),
        6 => u32::from_str_radix(hex, 16).ok(),
        _ => None,
    }
    .unwrap_or(0xff_ff_ff);
    // NaN clamps to the high end, which is the safe way to be wrong: opaque
    // rather than invisible.
    let alpha = if opacity.is_nan() {
        255
    } else {
        (opacity.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    format!("#{alpha:02X}{rgb:06X}")
}

/// No box at all. mpv keeps the last value it was given, so the backdrops
/// that draw no box must say so rather than leave the previous one's behind.
const TRANSPARENT: &str = "#00000000";

/// Formats a subtitle measurement for mpv, which takes these as floats.
/// Non-finite and negative values become zero: they are a settings bug, not
/// a reason for the property write to fail and take the rest with it.
fn sub_len(v: f64) -> String {
    let v = if v.is_finite() { v.max(0.0) } else { 0.0 };
    format!("{v}")
}

/// The mpv properties that express a subtitle style.
///
/// Every backdrop sets the same set of properties. mpv has no "reset to the
/// other backdrop's defaults", so switching from a box to an outline has to
/// zero the box explicitly or the box simply stays.
fn subtitle_properties(style: &crate::ipc::types::SubtitleStyleDto) -> Vec<(&'static str, String)> {
    use crate::ipc::types::SubtitleBackdrop as B;

    let font = match style.font.trim() {
        "" => "sans-serif",
        f => f,
    };
    // The opacity control belongs to the backdrop; the letters are opaque.
    // A translucent glyph over a translucent box reads as neither.
    let text = mpv_color(&style.color, 1.0);
    let backdrop = mpv_color("#000000", style.backdrop_opacity);

    let (border_style, back_color, outline, shadow) = match style.backdrop {
        B::Background => ("background-box", backdrop, 0.0, 0.0),
        B::Window => ("opaque-box", backdrop, 0.0, 0.0),
        B::Outline => (
            "outline-and-shadow",
            TRANSPARENT.to_owned(),
            style.outline_size,
            0.0,
        ),
        B::Shadow => (
            "outline-and-shadow",
            TRANSPARENT.to_owned(),
            0.0,
            style.shadow_offset,
        ),
        B::None => ("outline-and-shadow", TRANSPARENT.to_owned(), 0.0, 0.0),
    };

    vec![
        ("sub-font", font.to_owned()),
        ("sub-font-size", style.size.to_string()),
        ("sub-color", text),
        ("sub-border-style", border_style.to_owned()),
        ("sub-back-color", back_color),
        // `sub-border-size` in mpv's older documentation; this build reports
        // `sub-outline-size` as the canonical name and keeps the old one as
        // a deprecated alias.
        ("sub-outline-size", sub_len(outline)),
        ("sub-shadow-offset", sub_len(shadow)),
    ]
}

/// Bits per second of live video, measured on a 1080p59.94 feed from this
/// panel . Every stream sampled was H.264 SDR at roughly
/// this rate, so it is the honest number to size a byte budget with — but it
/// is a *typical* rate, not a ceiling: a genuinely 4K channel would fill the
/// budget faster and rewind correspondingly less far.
const LIVE_BITS_PER_SEC: f64 = 18_200_000.0;

/// Roughly fourteen seconds at the measured rate. `demuxer-max-back-bytes=0`
/// does not mean "no rewind limit", it means no back buffer at all, and mpv
/// then refuses to seek back even the second it is holding — which reads as
/// a broken player rather than as a setting.
const MIN_BACK_BYTES: i64 = 32 * 1024 * 1024;

/// The back buffer is resident memory. Two gigabytes is about eight minutes
/// of the measured feed and as much as this is willing to take from the rest
/// of the machine.
const MAX_BACK_BYTES: i64 = 2 * 1024 * 1024 * 1024;

/// Minutes of scrollback as the byte budget mpv actually wants.
fn rewind_bytes(minutes: f64) -> i64 {
    // NaN is a broken setting and takes the floor; an infinity is someone
    // asking for everything and takes the ceiling.
    if minutes.is_nan() || minutes <= 0.0 {
        return MIN_BACK_BYTES;
    }
    let bytes = minutes * 60.0 * LIVE_BITS_PER_SEC / 8.0;
    // Through f64 rather than a cast on an unbounded value: `as` saturates,
    // but the clamp is the thing being tested and it should be visible.
    if bytes >= MAX_BACK_BYTES as f64 {
        return MAX_BACK_BYTES;
    }
    (bytes.round() as i64).clamp(MIN_BACK_BYTES, MAX_BACK_BYTES)
}

/// The mpv properties that express the playback settings.
fn playback_properties(s: &crate::ipc::types::SettingsDto) -> Vec<(&'static str, String)> {
    vec![
        (
            "demuxer-max-back-bytes",
            rewind_bytes(s.rewind_minutes).to_string(),
        ),
        (
            "hwdec",
            // `auto-safe` rather than `auto`: the same value mpv is
            // initialised with, which excludes the decoders that fail
            // silently on this hardware.
            match s.hardware_decode {
                true => "auto-safe",
                false => "no",
            }
            .to_owned(),
        ),
        (
            "tone-mapping",
            // mpv's "off" for a tone curve is `clip`, not `no`.
            match s.tone_map_hdr {
                true => "auto",
                false => "clip",
            }
            .to_owned(),
        ),
    ]
}

/// Sets every property, then reports. One property mpv dislikes must not stop
/// the rest: a font it cannot find should not also cost you hardware decoding.
fn apply_properties(p: &Player, props: &[(&'static str, String)]) -> Result<(), String> {
    let failures: Vec<String> = props
        .iter()
        .filter_map(|(k, v)| match p.set_property(k, v) {
            Ok(()) => None,
            Err(e) => Some(format!("{k}: {e}")),
        })
        .collect();
    match failures.is_empty() {
        true => Ok(()),
        false => Err(failures.join("; ")),
    }
}

/// Applies subtitle styling to the running player.
///
/// Subtitles are drawn by libass inside the video layer, not the DOM, so this
/// is the only way to style them.
#[tauri::command]
pub async fn set_subtitle_style(
    app: AppHandle,
    style: crate::ipc::types::SubtitleStyleDto,
) -> Result<(), String> {
    let props = subtitle_properties(&style);
    on_player(&app, move |p| apply_properties(p, &props)).await
}

/// Applies playback settings that mpv owns: rewind budget, hardware decode,
/// tone mapping. Persisted by the caller.
#[tauri::command]
pub async fn apply_playback_settings(
    app: AppHandle,
    settings: crate::ipc::types::SettingsDto,
) -> Result<(), String> {
    let props = playback_properties(&settings);
    on_player(&app, move |p| apply_properties(p, &props)).await
}

/// Applies the settings on disk to the player, once, at startup.
///
/// Settings that only take effect when you change them are settings that
/// silently reset every launch. `lib.rs` calls this from `setup`, after the
/// player is built.
///
/// Deliberately infallible and non-blocking: it queues onto the main
/// thread and returns, so it is safe to call from `setup` (which is already
/// on that thread) without deadlocking on itself. Nothing here is worth
/// failing a launch over — the defaults mpv was initialised with are a
/// working player.
pub fn apply_saved_settings(app: &tauri::AppHandle) {
    let saved = crate::catalog::settings::load_or_default();
    let mut props = playback_properties(&saved);
    props.extend(subtitle_properties(&saved.subtitle));

    if let Err(e) = app.run_on_main_thread(move || {
        match mpv::with_player(|p| apply_properties(p, &props)) {
            None => tracing::warn!("no player yet; saved settings were not applied"),
            Some(Err(e)) => tracing::warn!("some saved settings were not applied: {e}"),
            Some(Ok(())) => tracing::info!("saved settings applied"),
        }
    }) {
        tracing::warn!("could not reach the main thread to apply saved settings: {e}");
    }


}
