//! Playback control.
//!
//! Many accounts allow only ONE concurrent connection, so every path here
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
use crate::ipc::ending::{self, Clues, How};
use crate::ipc::tracks;
use crate::ipc::types::{
    events, AspectMode, DeinterlaceMode, LiveJumpDto, PlaybackState, PlayerOptionsDto,
    PlayerStateDto,
};
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
    /// The one catch-up after a channel's connect burst; see [`Settle`].
    settle: Option<Settle>,
    /// Automatic catch-ups so far this session, for [`LiveJumpDto::seq`].
    jumps: u64,
    /// Whether this stream is live rather than a catch-up programme. A
    /// programme that plays to its end is finished; a live stream that ends
    /// was ended by somebody.
    live: bool,
    /// Whether a frame of this stream has been on screen: the difference
    /// between a stream that was taken away and one that never came.
    shown: bool,
    /// What mpv has logged about this stream; see `ipc::ending`.
    clues: Clues,
}

/// Brings a freshly tuned channel up to live once its connect burst is over.
///
/// A server answers a new connection with a burst of what it already holds,
/// and mpv starts at the oldest frame of it: tuning in landed 20-odd seconds
/// in the past and stayed there, because the automatic catch-up only answers
/// time *lost* to stalls — deliberately, since chasing the edge while the
/// burst was still arriving would seek, wait, and seek again: a
/// stutter-jump-stutter at the start of every zap.
///
/// So this waits for the burst to finish — `behind_live` stops growing for
/// [`SETTLE_QUIET`] — or for [`SETTLE_MAX`] if it never visibly does, and
/// then makes one decision: jump to the live cushion if playback is more
/// than [`SETTLE_SLACK`] past it, otherwise nothing. One seek, after the
/// data has arrived, so there is nothing for it to stutter over.
#[derive(Debug, Clone, Copy)]
struct Settle {
    /// Seconds since the first frame.
    age: f64,
    /// The furthest behind live seen so far.
    peak: f64,
    /// Seconds since `peak` last grew.
    quiet: f64,
}

/// How long `behind_live` must stop growing for the burst to count as over.
const SETTLE_QUIET: f64 = 1.5;
/// The longest to wait for that before deciding anyway.
const SETTLE_MAX: f64 = 10.0;
/// How far past the cushion is worth a seek. Less than this is the
/// sawtooth of an ordinary live stream, not a burst's backlog.
const SETTLE_SLACK: f64 = 2.0;
/// Growth smaller than this is the seekable end ticking over, not the burst.
const SETTLE_GROWTH: f64 = 0.5;

impl Settle {
    fn new() -> Self {
        Self {
            age: 0.0,
            peak: 0.0,
            quiet: 0.0,
        }
    }

    /// Takes one heartbeat (`dt` seconds since the last) and the current
    /// distance behind live. `None` while waiting; `Some(by)` once decided,
    /// where `by` is the forward seek to make (zero: none needed).
    fn observe(&mut self, dt: f64, behind: f64) -> Option<f64> {
        self.age += dt;
        if behind > self.peak + SETTLE_GROWTH {
            self.peak = behind;
            self.quiet = 0.0;
        } else {
            self.quiet += dt;
        }
        if self.quiet < SETTLE_QUIET && self.age < SETTLE_MAX {
            return None;
        }
        Some(if behind > LIVE_CUSHION + SETTLE_SLACK {
            behind - LIVE_CUSHION
        } else {
            0.0
        })
    }
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
            settle: None,
            jumps: 0,
            live: true,
            shown: false,
            clues: Clues::default(),
        }
    }

    /// A live channel is opening. A catch-up programme sets `live` false
    /// straight after.
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
        self.dto.ending = None;
        self.live = true;
        self.shown = false;
        self.clues = Clues::default();
        // The previous stream's measurements are not this stream's. Clearing
        // them is what makes the quality readout honest across a zap.
        self.dto.width = None;
        self.dto.height = None;
        self.dto.fps = None;
        self.dto.codec = None;
        self.dto.behind_live = 0.0;
        self.dto.buffered_back = 0.0;
        self.opened_at = Some(Instant::now());
        self.settle = None;
        // The last stream's catch-up is not news about this one.
        self.dto.live_jump = None;
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
        self.dto.ending = None;
        self.shown = true;
        // Only on the way out of Opening: a later reconfigure (a resolution
        // change mid-stream) is not a new connection and has no burst.
        if self.opened_at.is_some() {
            self.settle = Some(Settle::new());
        }
        self.opened_at = None;
    }

    fn set_paused(&mut self, paused: bool) {
        match self.dto.state {
            PlaybackState::Playing if paused => self.dto.state = PlaybackState::Paused,
            PlaybackState::Paused if !paused => self.dto.state = PlaybackState::Playing,
            _ => {}
        }
    }

    /// Failed, with words but no account of why: the open itself could not
    /// be sent. [`Machine::ended_with`] is the failure mpv reports.
    fn failed(&mut self, why: impl Into<String>) {
        self.following_live = false;
        self.dto.state = PlaybackState::Failed;
        self.dto.error = Some(why.into());
        self.dto.ending = None;
        self.opened_at = None;
    }

    fn stopped(&mut self) {
        self.dto.at_live_edge = true;
        self.following_live = false;
        self.dto.state = PlaybackState::Stopped;
        self.dto.stream_id = None;
        self.dto.error = None;
        self.dto.ending = None;
        self.dto.behind_live = 0.0;
        self.dto.buffered_back = 0.0;
        self.opened_at = None;
        self.dto.live_jump = None;
    }

    /// mpv ended the file on its own, without being asked to.
    fn ended(&mut self) {
        self.ended_with(How::Eof);
    }

    /// The stream stopped by itself: say why, and stay stopped.
    ///
    /// Failed rather than stopped, even for a live stream that played for an
    /// hour, so the interface has something to show: a picture that silently
    /// vanished is what "I got kicked off and it didn't tell me" was. The
    /// stream id is kept, so Retry knows what to tune. Nothing here tunes it
    /// again — see `ipc::ending` for why that must only ever be a person's
    /// decision.
    ///
    /// A catch-up programme that reached its end is the exception: it is
    /// over, and stopping is the right answer.
    fn ended_with(&mut self, how: How) {
        // Past the first-frame deadline mpv is still trying, and when it
        // gives up its reason is better than "no video".
        let refining = self.dto.state == PlaybackState::Failed
            && how != How::Deadline
            && self.dto.ending.is_some_and(|e| e.kind == ending::EndingKind::NoVideo);
        if !refining
            && !matches!(
                self.dto.state,
                PlaybackState::Opening | PlaybackState::Playing | PlaybackState::Paused
            )
        {
            return;
        }
        self.dto.at_live_edge = true;
        self.following_live = false;
        if how == How::Eof && self.shown && !self.live {
            self.stopped();
            return;
        }
        let why = match how {
            How::Error(code) => ending::error_text(code),
            How::Eof if self.shown => "the stream ended".to_owned(),
            How::Eof => "the stream ended before a frame arrived".to_owned(),
            How::Deadline => format!("no video within {}s", FIRST_FRAME_DEADLINE.as_secs()),
        };
        self.failed(why);
        self.dto.ending = Some(ending::classify(how, self.shown, self.clues));
    }

    /// Playback is being taken `by` seconds forward to live, by the app
    /// rather than the viewer. Bookkeeping only: the caller makes the seek.
    ///
    /// Resets the drift measure and stamps the nudge clock, which both
    /// automatic catch-ups need, and reports the jump so the frontend can say
    /// what just happened to the picture.
    fn caught_up(&mut self, by: f64) {
        self.last_live_nudge = Some(Instant::now());
        self.drift_secs = 0.0;
        self.jumps += 1;
        self.dto.live_jump = Some(LiveJumpDto {
            seconds: round_tenth(by),
            seq: self.jumps,
        });
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
            self.ended_with(How::Deadline);
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
    let newly_failed;
    // The newest track list this tick, if it moved. Only the last matters.
    let tracks = signals.iter().rev().find_map(|s| match s {
        Signal::Tracks(json) => Some(json.as_str()),
        _ => None,
    });
    if let Some(json) = tracks {
        let (audio, subtitles) = tracks::parse_track_list(json);
        commit_options(app, |o| {
            o.audio = audio;
            o.subtitles = subtitles;
        });
    }
    let payload = {
        let mut m = STATE.lock().expect("player state mutex poisoned");
        let was_failed = m.dto.state == PlaybackState::Failed;
        for s in &signals {
            match s {
                Signal::VideoReconfig => {
                    m.playing();
                    // Which decoder is doing the work, once per reconfigure.
                    // Software decoding is the difference between a busy CPU
                    // being unnoticeable and it dropping frames and starving
                    // the audio, and it falls back silently — so it is said.
                    tracing::info!(
                        "decoding {} with {}",
                        p.get_string("video-codec").unwrap_or_else(|| "video".into()),
                        p.get_string("hwdec-current")
                            .map_or_else(|| "software (no hardware decoder)".into(), |h| format!("hardware ({h})"))
                    );
                }
                Signal::EndFile { error: Some(code), .. } => m.ended_with(How::Error(*code)),
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
                // Evidence for why it might end; see `ipc::ending`. Only while
                // a stream is current: once it has stopped there is nothing
                // left to explain.
                Signal::Log(text) => {
                    if matches!(
                        m.dto.state,
                        PlaybackState::Opening | PlaybackState::Playing | PlaybackState::Paused
                    ) {
                        m.clues.observe(text);
                    }
                }
                Signal::StartFile | Signal::FileLoaded | Signal::Tracks(_) => {}
            }
        }
        m.apply(&probe);
        m.check_deadline();
        // Only a stream that never showed a picture: the tuner is asked again,
        // which is a connection, and one that ended mid-play is not a busy
        // tuner to explain but a stream to leave alone.
        newly_failed = if !was_failed && m.dto.state == PlaybackState::Failed && !m.shown {
            m.dto.stream_id
        } else {
            None
        };

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

            // Once per tune-in: up to live after the connect burst, see
            // `Settle`. It runs before the drift rule below and resets it,
            // so the two cannot both seek on one tick.
            if let Some(decision) = m.settle.as_mut().and_then(|s| s.observe(elapsed, probe.behind_live)) {
                m.settle = None;
                if decision > 0.05 {
                    tracing::info!(
                        "tuned in {:.1}s behind live; catching up to the cushion",
                        probe.behind_live
                    );
                    m.caught_up(decision);
                    let _ = p.command("seek", &[&format!("{decision:.3}"), "relative"]);
                }
            }

            let settled = m
                .last_live_nudge
                .is_none_or(|t| t.elapsed() >= LIVE_NUDGE_INTERVAL);
            if m.drift_secs > MAX_DRIFT_SECS && settled && probe.behind_live > LIVE_CUSHION + 1.0 {
                let by = (probe.behind_live - LIVE_CUSHION).max(0.0);
                m.caught_up(by);
                // Best-effort: failing to catch up is not a reason to fail a
                // stream that is otherwise playing fine.
                let _ = p.command("seek", &[&format!("{by:.3}"), "relative"]);
            }
        } else {
            m.drift_secs = 0.0;
            // Rewound on purpose (or paused): being behind is the point now.
            if !m.following_live {
                m.settle = None;
            }
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
    if let Some(id) = newly_failed {
        explain_failure(app, id);
    }
    if let Some(e) = earned {
        persist(e);
    }
    if let Some(payload) = payload {
        let _ = app.emit(events::PLAYER_STATE, payload);
    }
}

/// Asks the account why a channel failed, when it can say more than mpv.
///
/// mpv reports a refused stream as "loading failed". An HDHomeRun knows
/// whether that was every tuner being busy, or no signal, and the person
/// looking at the error needs to know which: one is fixed by stopping a
/// recording, the other by nothing at all. The question goes to the tuner
/// off the main thread, and the answer replaces mpv's message only if the
/// same channel is still the one that failed — a zap in the meantime is a
/// different story.
///
/// Only for a source already built: the heartbeat is no place to read the
/// account file, and a failure on a source nobody has built yet cannot be
/// an HDHomeRun stream.
fn explain_failure(app: &AppHandle, stream_id: i64) {
    let cached = CLIENT.read().ok().and_then(|slot| *slot);
    let Some(Source::HdHomeRun(tuner)) = cached else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(why) = tuner.explain_failure(stream_id).await else {
            return;
        };
        commit(&app, |m| {
            if m.dto.stream_id == Some(stream_id) && m.dto.state == PlaybackState::Failed {
                m.dto.error = Some(why);
                // The tuner's own words name the cause and the fix; a
                // general guess about providers would only talk over them.
                m.dto.ending = None;
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Credentials and URLs
// ---------------------------------------------------------------------------

/// Development override pointing playback at a locally served fixture.
///
/// Many accounts allow only one concurrent connection and real streams are
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
pub fn forget_source() {
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
pub fn xtream() -> Result<&'static XtreamClient, String> {
    match source()? {
        Source::Xtream(c) => Ok(c),
        other => Err(format!(
            "the account is {}, not an Xtream server",
            other.kind().label()
        )),
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
    open(&app, stream_id, target, true).await
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
        Source::Xtream(server) => {
            // The server reads the timestamp in its own wall clock, which is
            // the machine's: sending UTC replays the wrong hour for most of
            // the world.
            let stamp = timeshift_stamp(start, local_utc_offset(start));
            Playable::plain(server.timeshift_url(stream_id, &stamp, duration_minutes))
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
        // A tuner has no archive, and a Tvheadend server's is its DVR, which
        // is not read. Their channels are never marked as having catch-up,
        // so this is only reached by a stale request from before a switch.
        Source::HdHomeRun(_) | Source::Tvheadend(_) => {
            return Err("this account has no catch-up".to_owned());
        }
    };
    open(&app, stream_id, target, false).await
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
async fn open(app: &AppHandle, stream_id: i64, target: Playable, live: bool) -> Result<(), String> {
    ensure_ticker(app);
    // The scrollback previews describe a buffer that is about to be closed.
    // Keeping them would put the previous channel's picture under the cursor.
    crate::surface::trickplay::clear();
    // Same reasoning, for the same reason: the graphs describe one stream.
    crate::ipc::health::clear();
    commit(app, |m| {
        m.opening(stream_id);
        m.live = live;
    });
    let r = on_player(app, move |p| {
        apply_http_headers(p, &target);
        reset_track_choice(p);
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

/// How far behind the newest data "live" sits: where `seek_live` lands, the
/// furthest a forward skip goes, and where the automatic catch-up aims.
///
/// Half a second — just enough not to aim past a boundary that moves while
/// it is being read — is not enough: every jump to live stalls a few seconds
/// later. A server does not deliver evenly: it goes quiet for a second
/// or two and then sends what it owes in a burst, so half a second of video
/// ahead of the playhead runs dry in the first quiet spell and mpv stops to
/// refill. Measured with `player_check` against `fake_stream.py --gap-every
/// 4` (1.5s gaps): at 0.5s, one stall in the fifteen seconds after reaching
/// live, the buffer ahead drained to 0.0s; at this value, none.
///
/// Four seconds is a delay nobody watching live TV notices, and comfortably
/// more than the gaps a server leaves.
pub const LIVE_CUSHION: f64 = 4.0;

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
///
/// Measured from the newest data, so it sits above [`LIVE_CUSHION`]: playback
/// parked on the cushion is live, which is where `seek_live` puts it.
const LIVE_ENTER: f64 = LIVE_CUSHION + 3.0;
/// Above this, it is considered to have fallen behind.
///
/// The gap between the two is the point. `behind_live` is measured against the
/// end of mpv's seekable window, which advances in chunks as the demuxer
/// reads, so it sawtooths by a couple of seconds while playback is perfectly
/// live. A single threshold makes the indicator flicker between "live" and a
/// few seconds behind; two thresholds mean it only changes when something
/// actually changed.
const LIVE_EXIT: f64 = LIVE_CUSHION + 8.0;

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
        let by = (probe.behind_live - LIVE_CUSHION).max(0.0);
        match by > 0.05 {
            true => p.command("seek", &[&format!("{by:.3}"), "relative"]),
            false => Ok(()),
        }
        .map_err(|e| e.to_string())
    })
    .await
}

/// How loud a volume setting *sounds*, as mpv's own volume.
///
/// The slider is in perceived loudness: 70% should sound about 70% as loud,
/// and 200% about twice as loud. mpv's `volume` is not that: it is cubed into
/// an amplitude (`pow(volume / 100, 3)`, player/audio.c), so 70% was 0.34 of
/// full amplitude, −9.3 dB, which sounds like about half — "70% sounds like
/// half volume", as it was put. Loudness grows roughly as amplitude^0.6
/// (Stevens), so a perceived fraction `p` wants amplitude `p^(1/0.6)`, and
/// mpv reaches that with `100 · p^(1/(3·0.6))` = `100 · p^(5/9)`.
///
/// 100 maps to 100, so mpv's default and the slider agree at unity; above it
/// the same curve boosts, 200% landing at +10 dB rather than mpv's +18.
fn mpv_volume(perceived: f64) -> f64 {
    if !perceived.is_finite() || perceived <= 0.0 {
        return 0.0;
    }
    100.0 * (perceived / 100.0).powf(5.0 / 9.0)
}

/// 0-200; above 100 is software boost via `volume-max`.
#[tauri::command]
pub async fn set_volume(app: AppHandle, volume: f64) -> Result<(), String> {
    let v = volume.clamp(0.0, 200.0);
    let to_mpv = mpv_volume(v);
    on_player(&app, move |p| {
        p.set_f64("volume", to_mpv).map_err(|e| e.to_string())
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
            stalls: crate::ipc::health::stalls(),
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
// The picture menu: tracks, aspect, deinterlacing
// ---------------------------------------------------------------------------

/// What the picture menu shows. The track lists come from mpv's own
/// `track-list` notifications (see the heartbeat); the aspect and the
/// deinterlacing are what was last chosen, which mpv keeps across channel
/// changes because both are global options rather than per-file ones.
static OPTIONS: LazyLock<Mutex<PlayerOptionsDto>> =
    LazyLock::new(|| Mutex::new(PlayerOptionsDto::default()));

/// Changes the picture menu's state and pushes it if anything moved.
fn commit_options(app: &AppHandle, f: impl FnOnce(&mut PlayerOptionsDto)) -> PlayerOptionsDto {
    let (now, changed) = {
        let mut o = OPTIONS.lock().expect("player options mutex poisoned");
        let before = o.clone();
        f(&mut o);
        (o.clone(), *o != before)
    };
    if changed {
        let _ = app.emit(events::PLAYER_OPTIONS, now.clone());
    }
    now
}

/// A track picked on one channel is not a choice for the next.
///
/// `aid` and `sid` are options as well as properties: setting one switches
/// the track *and* stays set for every file after it. Audio track 2 on the
/// next channel may be a different language, or not exist — and mpv then
/// plays that channel silent. So each stream opens on mpv's own choice.
fn reset_track_choice(p: &Player) {
    for key in ["aid", "sid"] {
        if let Err(e) = p.set_property(key, "auto") {
            tracing::warn!("could not reset {key}: {e}");
        }
    }
}

#[tauri::command]
pub async fn player_options() -> Result<PlayerOptionsDto, String> {
    Ok(OPTIONS.lock().expect("player options mutex poisoned").clone())
}

/// Switches to one of the stream's audio tracks, by mpv's audio id.
#[tauri::command]
pub async fn set_audio_track(app: AppHandle, id: i64) -> Result<PlayerOptionsDto, String> {
    on_player(&app, move |p| {
        p.set_property("aid", &tracks::track_value(Some(id)))
            .map_err(|e| e.to_string())
    })
    .await?;
    // The list itself follows from mpv's notification; this is so the menu
    // that asked sees its answer without waiting a heartbeat for it.
    Ok(commit_options(&app, |o| {
        for t in &mut o.audio {
            t.selected = t.id == id;
        }
    }))
}

/// Shows one of the stream's subtitle tracks, or none.
#[tauri::command]
pub async fn set_subtitle_track(
    app: AppHandle,
    id: Option<i64>,
) -> Result<PlayerOptionsDto, String> {
    on_player(&app, move |p| {
        p.set_property("sid", &tracks::track_value(id))
            .map_err(|e| e.to_string())
    })
    .await?;
    Ok(commit_options(&app, |o| {
        for t in &mut o.subtitles {
            t.selected = Some(t.id) == id;
        }
    }))
}

#[tauri::command]
pub async fn set_aspect(app: AppHandle, aspect: AspectMode) -> Result<PlayerOptionsDto, String> {
    let props: Vec<(&'static str, String)> = tracks::aspect_properties(aspect)
        .iter()
        .map(|(k, v)| (*k, (*v).to_owned()))
        .collect();
    on_player(&app, move |p| apply_properties(p, &props)).await?;
    Ok(commit_options(&app, |o| o.aspect = aspect))
}

#[tauri::command]
pub async fn set_deinterlace(
    app: AppHandle,
    mode: DeinterlaceMode,
) -> Result<PlayerOptionsDto, String> {
    on_player(&app, move |p| {
        p.set_property("deinterlace", tracks::deinterlace_value(mode))
            .map_err(|e| e.to_string())
    })
    .await?;
    Ok(commit_options(&app, |o| o.deinterlace = mode))
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// Clamps a relative seek to what is actually buffered.
///
/// Seeking past either end of the demuxer cache on a live stream drops the
/// connection and reopens it, which a single-connection account cannot afford.
/// Forward, it stops at [`LIVE_CUSHION`] rather than the very edge: a skip
/// that lands on the newest data has nothing ahead of it and stalls a few
/// seconds later, exactly as a `seek_live` to the very edge would. Already on
/// the cushion, a forward skip does nothing.
fn clamp_seek(seconds: f64, buffered_back: f64, behind_live: f64) -> f64 {
    if seconds < 0.0 {
        seconds.max(-buffered_back)
    } else {
        seconds.min((behind_live - LIVE_CUSHION).max(0.0))
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
    fn an_automatic_catch_up_is_reported_with_its_distance() {
        let mut m = Machine::new();
        m.opening(1);
        m.playing();
        assert_eq!(m.dto.live_jump, None, "nothing has jumped yet");
        m.drift_secs = 3.0;
        m.caught_up(12.04);
        assert_eq!(m.dto.live_jump, Some(LiveJumpDto { seconds: 12.0, seq: 1 }));
        assert_eq!(m.drift_secs, 0.0, "the jump makes back what was lost");
        assert!(m.last_live_nudge.is_some(), "and holds off the next one");
        assert!(m.changed().is_some_and(|d| d.live_jump.is_some()));
    }

    #[test]
    fn a_second_jump_of_the_same_size_is_still_news() {
        let mut m = Machine::new();
        m.opening(1);
        m.playing();
        m.caught_up(6.0);
        let first = m.changed().and_then(|d| d.live_jump).expect("first jump");
        m.caught_up(6.0);
        let second = m.changed().and_then(|d| d.live_jump).expect("second jump");
        assert_eq!(first.seconds, second.seconds);
        assert!(second.seq > first.seq, "the counter is what tells them apart");
    }

    #[test]
    fn a_jump_belongs_to_the_stream_it_happened_on() {
        let mut m = Machine::new();
        m.opening(1);
        m.playing();
        m.caught_up(9.0);
        m.opening(2);
        assert_eq!(m.dto.live_jump, None, "a zap starts with no news");
        m.playing();
        m.caught_up(4.0);
        m.stopped();
        assert_eq!(m.dto.live_jump, None);
        // Counted across the session, so a jump on the next stream cannot
        // repeat a number the frontend has already announced.
        m.opening(3);
        m.playing();
        m.caught_up(4.0);
        assert_eq!(m.dto.live_jump.map(|j| j.seq), Some(3));
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
    fn a_live_stream_that_ends_mid_play_says_so_and_keeps_its_channel() {
        let mut m = Machine::new();
        m.opening(42);
        m.playing();
        m.ended();
        assert_eq!(m.dto.state, PlaybackState::Failed);
        assert_eq!(m.dto.stream_id, Some(42), "Retry needs to know what to tune");
        let e = m.dto.ending.expect("an explanation");
        assert_eq!(e.kind, ending::EndingKind::Dropped);
    }

    #[test]
    fn a_catch_up_programme_that_reaches_its_end_just_stops() {
        let mut m = Machine::new();
        m.opening(42);
        m.live = false;
        m.playing();
        m.ended();
        assert_eq!(m.dto.state, PlaybackState::Stopped);
        assert!(m.dto.ending.is_none());
    }

    #[test]
    fn a_refusal_carries_the_status_mpv_logged() {
        let mut m = Machine::new();
        m.opening(42);
        m.clues.observe("http: HTTP error 509 Max Connections Reached");
        m.ended_with(How::Error(-13));
        assert_eq!(m.dto.state, PlaybackState::Failed);
        assert_eq!(m.dto.error.as_deref(), Some("the stream could not be opened"));
        let e = m.dto.ending.unwrap();
        assert_eq!((e.kind, e.status, e.limit), (ending::EndingKind::Refused, Some(509), true));
    }

    #[test]
    fn a_new_channel_forgets_the_last_ones_ending() {
        let mut m = Machine::new();
        m.opening(1);
        m.clues.observe("HTTP error 403 Forbidden");
        m.ended_with(How::Error(-13));
        m.opening(2);
        assert!(m.dto.ending.is_none());
        assert_eq!(m.clues, Clues::default(), "the evidence was about the other stream");
        m.playing();
        m.ended();
        assert_eq!(m.dto.ending.unwrap().status, None);
    }

    #[test]
    fn mpvs_reason_refines_the_deadline_but_nothing_else_is_overwritten() {
        let mut m = Machine::new();
        m.opening(1);
        m.ended_with(How::Deadline);
        assert_eq!(m.dto.ending.unwrap().kind, ending::EndingKind::NoVideo);
        m.clues.observe("HTTP error 403 Forbidden");
        m.ended_with(How::Error(-13));
        assert_eq!(m.dto.ending.unwrap().kind, ending::EndingKind::Refused);
        // A failure already explained stays explained.
        m.ended();
        assert_eq!(m.dto.ending.unwrap().kind, ending::EndingKind::Refused);
    }

    #[test]
    fn stopping_clears_the_explanation() {
        let mut m = Machine::new();
        m.opening(1);
        m.playing();
        m.ended();
        m.stopped();
        assert!(m.dto.ending.is_none());
        assert!(m.dto.error.is_none());
    }

    #[test]
    fn seeks_are_clamped_to_the_buffered_window() {
        assert_eq!(clamp_seek(-5.0, 60.0, 3.0), -5.0);
        assert_eq!(clamp_seek(-90.0, 60.0, 3.0), -60.0);
        assert_eq!(clamp_seek(-5.0, 0.0, 0.0), 0.0);
    }

    /// Feeds `Settle` a series of `behind_live` readings a heartbeat apart and
    /// returns the decision and the heartbeat it came on.
    fn settle_run(readings: impl IntoIterator<Item = f64>) -> Option<(usize, f64)> {
        let mut s = Settle::new();
        let dt = TICK.as_secs_f64();
        readings
            .into_iter()
            .enumerate()
            .find_map(|(i, b)| s.observe(dt, b).map(|by| (i, by)))
    }

    #[test]
    fn a_tune_in_burst_is_caught_up_once_it_has_finished() {
        // The burst: behind_live climbs to 24s over two seconds, then holds
        // (real-time delivery, the seekable end ticking over by tenths).
        let mut readings: Vec<f64> = (1..=8).map(|i| f64::from(i) * 3.0).collect();
        readings.extend((0..40).map(|i| 24.0 + f64::from(i % 3) * 0.1));
        let (at, by) = settle_run(readings).expect("never decided");
        // Not during the climb: the seek waits for the data to have arrived.
        assert!(at >= 8 + 5, "decided at heartbeat {at}, while the burst was arriving");
        // Lands on the cushion.
        assert!((24.0 - by - LIVE_CUSHION).abs() < 0.2, "jumped by {by}");
    }

    #[test]
    fn a_tune_in_already_near_live_is_left_alone() {
        let readings = std::iter::repeat_n(LIVE_CUSHION + 1.0, 40);
        assert_eq!(settle_run(readings).map(|(_, by)| by), Some(0.0));
    }

    #[test]
    fn a_burst_that_never_settles_is_decided_anyway() {
        // Still growing at SETTLE_MAX: decide then rather than wait forever.
        let readings = (1..=200).map(|i| f64::from(i) * 0.6);
        let (at, by) = settle_run(readings).expect("never decided");
        let dt = TICK.as_secs_f64();
        assert!((at as f64 + 1.0) * dt >= SETTLE_MAX - dt);
        assert!(by > 0.0);
    }

    /// The gain mpv applies for one of its volumes, in dB.
    fn mpv_db(mpv: f64) -> f64 {
        20.0 * (mpv / 100.0).powi(3).log10()
    }

    #[test]
    fn the_volume_slider_tracks_how_loud_it_sounds() {
        // Unity is unity: mpv's default and the slider agree.
        assert!((mpv_volume(100.0) - 100.0).abs() < 1e-9);
        // 70% was −9.3 dB through mpv's cube, which sounds like half. Now
        // about −5 dB, which sounds like about 70%: loudness ≈ amplitude^0.6.
        let db70 = mpv_db(mpv_volume(70.0));
        assert!((-5.6..=-4.8).contains(&db70), "70% is {db70:.1} dB");
        let perceived = 10f64.powf(db70 / 20.0).powf(0.6);
        assert!((perceived - 0.7).abs() < 0.01, "70% sounds like {perceived:.2}");
        // Boost: 200% about twice as loud (+10 dB), not mpv's +18.
        let db200 = mpv_db(mpv_volume(200.0));
        assert!((9.5..=10.5).contains(&db200), "200% is {db200:.1} dB");
        // Silence and nonsense are silence, and it never decreases.
        assert_eq!(mpv_volume(0.0), 0.0);
        assert_eq!(mpv_volume(f64::NAN), 0.0);
        let mut last = 0.0;
        for v in (0..=200).map(f64::from) {
            let m = mpv_volume(v);
            assert!(m >= last);
            last = m;
        }
    }

    #[test]
    fn a_forward_skip_stops_on_the_cushion_not_the_edge() {
        // 20s behind: +5 is a full 5.
        assert_eq!(clamp_seek(5.0, 60.0, 20.0), 5.0);
        // 6s behind: only as far as the cushion, never onto the newest data.
        assert_eq!(clamp_seek(5.0, 60.0, LIVE_CUSHION + 2.0), 2.0);
        // Already on the cushion, or closer: nowhere to go.
        assert_eq!(clamp_seek(5.0, 60.0, LIVE_CUSHION), 0.0);
        assert_eq!(clamp_seek(5.0, 60.0, 1.0), 0.0);
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
    fn the_rewind_budget_is_the_megabytes_asked_for() {
        assert_eq!(rewind_bytes(512), 512_000_000);
        assert_eq!(rewind_bytes(546), 546_000_000);
    }

    #[test]
    fn a_rewind_of_zero_is_a_floor_not_a_disabled_cache() {
        // demuxer-max-back-bytes=0 does not mean "no rewind", it means no
        // back buffer at all - and mpv then cannot seek back even the second
        // it already has, which reads as a broken player rather than a
        // setting.
        assert_eq!(rewind_bytes(0), MIN_BACK_BYTES);
        assert_eq!(rewind_bytes(1), MIN_BACK_BYTES);
    }

    #[test]
    fn an_absurd_rewind_is_capped_rather_than_swapping_the_machine() {
        assert_eq!(rewind_bytes(100_000), MAX_BACK_BYTES);
        assert_eq!(rewind_bytes(u32::MAX), MAX_BACK_BYTES);
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
            rewind_mb: 273,
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
        // Parked on the cushion, behind_live sawtooths a few seconds either
        // side of it; that is normal live playback and the answer must not
        // change through it.
        let c = LIVE_CUSHION;
        let mut live = true;
        for behind in [c, c + 2.4, c - 1.8, c + 3.0, c + 0.1, c + 2.9, c - 2.0] {
            live = live_edge_with_hysteresis(live, behind);
            assert!(live, "went non-live at {behind}s, which is still live");
        }
    }

    #[test]
    fn seek_live_lands_somewhere_the_indicator_calls_live() {
        assert!(live_edge_with_hysteresis(false, LIVE_CUSHION));
    }

    #[test]
    fn a_real_rewind_leaves_the_live_edge_and_stays_left() {
        let c = LIVE_CUSHION;
        let mut live = true;
        live = live_edge_with_hysteresis(live, c + 12.0);
        assert!(!live, "12s past the cushion is not live");
        // Coming back must clear the tighter entry threshold, not merely dip
        // under the exit one, or it would flap around the exit.
        live = live_edge_with_hysteresis(live, c + 5.0);
        assert!(!live, "under the exit threshold but over the entry one");
        live = live_edge_with_hysteresis(live, c + 1.0);
        assert!(live, "just past the cushion is live again");
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

/// The floor. `demuxer-max-back-bytes=0` does not mean "no rewind limit", it
/// means no back buffer at all, and mpv then refuses to seek back even the
/// second it is holding — which reads as a broken player, not a setting.
const MIN_BACK_BYTES: i64 = 32 * 1_000_000;

/// The back buffer is resident memory; two gigabytes is as much as this is
/// willing to take from the rest of the machine.
const MAX_BACK_BYTES: i64 = 2_000 * 1_000_000;

/// The rewind setting, in megabytes, as the byte budget mpv wants.
///
/// Megabytes in, bytes out, nothing assumed. Minutes at a typical 18.2 Mbps
/// would hold nearly eleven minutes on a 6.8 Mbps channel for a setting that
/// said four; the settings pane shows what the budget buys at common
/// bitrates instead.
fn rewind_bytes(megabytes: u32) -> i64 {
    (i64::from(megabytes) * 1_000_000).clamp(MIN_BACK_BYTES, MAX_BACK_BYTES)
}

/// The mpv properties that express the playback settings.
fn playback_properties(s: &crate::ipc::types::SettingsDto) -> Vec<(&'static str, String)> {
    vec![
        (
            "demuxer-max-back-bytes",
            rewind_bytes(s.rewind_mb).to_string(),
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

/// What the desktop's media controls show: the channel, its programme and
/// whether it is playing, or `None` with nothing on. Accepted whether or not
/// the controls are on, so turning them on shows what is already playing.
#[tauri::command]
pub async fn set_media_session(
    session: Option<crate::ipc::types::MediaSessionDto>,
) -> Result<(), String> {
    crate::media_keys::set_session(session);
    Ok(())
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
