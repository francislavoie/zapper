//! Playback control.
//!
//! Many accounts allow only a single connection, so every path here
//! must tear down before it opens. Never hold two streams, even briefly.
//! `loadfile ... replace` satisfies that by construction: mpv closes the
//! current demuxer before opening the next.
//!
//! **Threading.** Commands arrive on Tauri's async worker threads. The player
//! and the overlay are `!Send` and live in GTK-main-thread locals, so every
//! command that touches them hops with [`tauri::AppHandle::run_on_main_thread`]
//! and carries plain data across - never a GTK handle. The shared state that
//! both sides read is a `Mutex<Machine>` here, which is ordinary `Send` data.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};

use crate::ipc::types::{events, PlaybackState, PlayerStateDto};
use crate::surface::mpv::{self, Player, Probe, Signal};
use crate::surface::VideoRect;
use crate::xtream::{Credentials, XtreamClient};

/// How long a stream gets to produce its first frame before it is called dead.
/// Matches mpv's own `network-timeout`, set in [`Player::new`].
const FIRST_FRAME_DEADLINE: Duration = Duration::from_secs(8);

/// State-feed heartbeat. Fast enough that a channel change feels instant,
/// slow enough that it costs nothing.
const TICK: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------------------
// Main-thread access
// ---------------------------------------------------------------------------

/// Runs `f` on the GTK main thread and waits for its result.
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
    .map_err(|e| format!("could not reach the GTK main thread: {e}"))?;
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| "the GTK main thread did not answer".to_owned())?
}

/// Runs `f` on the GTK main thread and waits for its result.
/// Commands arrive on worker threads; the player is !Send and lives there.
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

/// Runs `f` against the overlay on the GTK main thread.
async fn on_overlay(
    app: &AppHandle,
    f: impl FnOnce(&crate::surface::overlay::Overlay) + Send + 'static,
) -> Result<(), String> {
    on_main(app, move || {
        crate::surface::with_overlay(f)
            .ok_or_else(|| "the video surface is not installed".to_owned())
    })
    .await
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Everything the frontend sees, plus the bookkeeping that decides when it
/// changes. Held behind a `Mutex` so worker threads and the GTK ticker can
/// both drive it.
struct Machine {
    dto: PlayerStateDto,
    /// When the current open began. The first-frame deadline runs off it.
    opened_at: Option<Instant>,
    /// The last payload actually emitted, so an unchanged tick stays silent.
    last_emitted: Option<String>,
    /// Whether this open still owes a jump to the live edge.
    ///
    /// The panel bursts on connect and mpv starts at the oldest cached frame,
    /// which lands playback ~20s in the past. For live TV that is simply
    /// wrong: you asked for the channel, not for what it showed half a minute
    /// ago. Cleared once the jump is made, or once the stream stops.
    live_seek_pending: bool,
}

impl Machine {
    fn new() -> Self {
        Self {
            dto: PlayerStateDto::default(),
            opened_at: None,
            last_emitted: None,
            live_seek_pending: false,
        }
    }

    fn opening(&mut self, stream_id: i64) {
        self.live_seek_pending = true;
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
        self.live_seek_pending = false;
        self.dto.state = PlaybackState::Failed;
        self.dto.error = Some(why.into());
        self.opened_at = None;
    }

    fn stopped(&mut self) {
        self.live_seek_pending = false;
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
        self.live_seek_pending = false;
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

/// One heartbeat, on the GTK main thread.
fn tick(app: &AppHandle, p: &Player) {
    let signals = p.pump();
    let probe = p.probe();
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

        // Land on the live edge, once, as soon as there is a picture.
        if m.live_seek_pending && m.dto.state == PlaybackState::Playing {
            m.live_seek_pending = false;
            let by = (probe.behind_live - LIVE_EDGE_MARGIN).max(0.0);
            if by > 1.0 {
                // Best-effort: failing to catch up is not a reason to fail a
                // stream that is otherwise playing fine.
                let _ = p.command("seek", &[&format!("{by:.3}"), "relative"]);
            }
        }

        m.changed()
    };
    if let Some(payload) = payload {
        let _ = app.emit(events::PLAYER_STATE, payload);
    }
}

// ---------------------------------------------------------------------------
// Credentials and URLs
// ---------------------------------------------------------------------------

/// Reads the first Xtream profile.
///
/// Delegates to `catalog::credentials`, which is the single loader: two
/// copies would drift, and this one is on the path that turns a profile into
/// a playable URL. Errors are mapped to plain strings that reach the UI, so
/// they must never carry a credential.
fn credentials() -> Result<Credentials, String> {
    crate::catalog::credentials::load().map_err(|e| e.to_string())
}

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

static CLIENT: OnceLock<XtreamClient> = OnceLock::new();

/// The process-wide panel client. Built once; a failure is not cached, so
/// fixing the profile does not need a restart.
///
/// Public so the verification harness can resolve a channel to play without
/// a second copy of the credential loader - and without a stream URL ever
/// reaching a command line, where it would be world-readable through `/proc`.
pub fn panel() -> Result<&'static XtreamClient, String> {
    if let Some(c) = CLIENT.get() {
        return Ok(c);
    }
    let c = XtreamClient::new(credentials()?).map_err(|e| e.to_string())?;
    Ok(CLIENT.get_or_init(|| c))
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn play(app: AppHandle, stream_id: i64) -> Result<(), String> {
    let url = match fixture_url(stream_id) {
        Some(u) => u,
        None => panel()?.live_url(stream_id),
    };
    open(&app, stream_id, url).await
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
    // The panel reads the timestamp in its own wall clock, which is the
    // machine's: sending UTC replays the wrong hour for most of the world.
    let stamp = timeshift_stamp(start, local_utc_offset(start));
    let url = panel()?.timeshift_url(stream_id, &stamp, duration_minutes);
    open(&app, stream_id, url).await
}

/// Shared by live and catchup: state first, then the one connection.
async fn open(app: &AppHandle, stream_id: i64, url: String) -> Result<(), String> {
    ensure_ticker(app);
    commit(app, |m| m.opening(stream_id));
    let r = on_player(app, move |p| {
        // `replace` closes the current demuxer before opening the next, which
        // is what keeps this a one-connection client.
        p.load(&url).map_err(|e| e.to_string())
    })
    .await;
    if let Err(e) = &r {
        commit(app, |m| m.failed(e.clone()));
    }
    r
}

#[tauri::command]
pub async fn stop(app: AppHandle) -> Result<(), String> {
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

#[tauri::command]
pub async fn seek_live(app: AppHandle) -> Result<(), String> {
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

/// Widely shared Web Audio parameters: threshold -50, knee 40, ratio 12, attack 0,
/// release 0.25.
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
    on_overlay(&app, move |o| o.set_video_rect(rect)).await
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

// A widely shared compressor, as authored for the Web Audio
// `DynamicsCompressorNode`. Kept in its original units so the numbers can be
// checked against the source they came from; `compressor_filter` converts.
const COMP_THRESHOLD_DB: f64 = -50.0;
const COMP_KNEE_DB: f64 = 40.0;
const COMP_RATIO: f64 = 12.0;
const COMP_ATTACK_S: f64 = 0.0;
const COMP_RELEASE_S: f64 = 0.25;

/// mpv's `af` value for the compressor.
///
/// ffmpeg's `acompressor` does not take Web Audio's units. Threshold is a
/// linear amplitude, not dB; attack and release are milliseconds, not
/// seconds; and knee is a linear width factor bounded to 1..8, so the published 40 dB
/// knee (a factor of 10) is clamped to the widest ffmpeg will accept - about
/// 36 dB, the closest reachable curve. Every bound below is from
/// `ffmpeg -h filter=acompressor`; exceeding one makes the filter fail to
/// load, which mpv reports as a bare property error.
fn compressor_filter() -> String {
    let threshold = db_to_amplitude(COMP_THRESHOLD_DB).clamp(0.000_976_563, 1.0);
    let knee = db_to_amplitude(COMP_KNEE_DB / 2.0).clamp(1.0, 8.0);
    let ratio = COMP_RATIO.clamp(1.0, 20.0);
    let attack = (COMP_ATTACK_S * 1000.0).clamp(0.01, 2000.0);
    let release = (COMP_RELEASE_S * 1000.0).clamp(0.01, 9000.0);
    format!(
        "acompressor=threshold={threshold:.6}:knee={knee:.3}:ratio={ratio}:\
         attack={attack}:release={release}"
    )
}

fn db_to_amplitude(db: f64) -> f64 {
    10f64.powf(db / 20.0)
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
/// is resolved out of the already-loaded libc - the same trick
/// [`crate::surface::mpv`] uses for `setlocale`. Only the offset is taken;
/// the calendar arithmetic stays in Rust where it can be tested.
fn local_utc_offset(unix: i64) -> i64 {
    use libloading::os::unix::Library;
    use std::ffi::{c_char, c_int, c_long};

    /// glibc's `struct tm`: nine ints, then `tm_gmtoff` and `tm_zone`.
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn compressor_uses_webaudio_values_in_ffmpegs_units() {
        // -50 dB is 0.003162 linear; 0.25 s is 250 ms; attack 0 is below
        // ffmpeg's 0.01 ms floor; a 40 dB knee is a factor of 10, above
        // ffmpeg's ceiling of 8.
        assert_eq!(
            compressor_filter(),
            "acompressor=threshold=0.003162:knee=8.000:ratio=12:attack=0.01:release=250"
        );
    }

    #[test]
    fn the_webaudio_constants_are_the_published_ones() {
        assert_eq!(
            (
                COMP_THRESHOLD_DB,
                COMP_KNEE_DB,
                COMP_RATIO,
                COMP_ATTACK_S,
                COMP_RELEASE_S
            ),
            (-50.0, 40.0, 12.0, 0.0, 0.25)
        );
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
}
