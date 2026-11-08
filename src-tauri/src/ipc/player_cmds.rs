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
use crate::catalog::credentials;
use crate::catalog::store;
use crate::catalog::vod_progress::{write_progress, ProgressSaver, ProgressWrite};
use crate::catalog::vod_store::{self, VodItem};
use crate::connections::{self, Limit, Verdict};
use crate::ipc::ending::{self, Clues, Ending, EndingKind, Flow, How};
use crate::ipc::live_start::{Join, Settle};
use crate::ipc::tracks;
use crate::ipc::types::{
    events, AspectMode, ChannelQualityDto, ConnectionsDto, DeinterlaceMode, LiveJumpDto,
    LiveStart, OnDemandDto, OnDemandKind, PlaybackState, PlayerOptionsDto, PlayerStateDto,
    ReconnectDto,
};
use crate::recording::client::{Client, PlayerWatch, Watching};
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

/// How long a live channel plays before its quality is read for the list.
/// Long enough for a measured frame rate to settle and for the audio to have
/// been decoded; short enough that a quick look at a channel still counts.
const MEASURE_AFTER: Duration = Duration::from_secs(5);

/// How long a failure that is only the first-frame deadline waits for mpv's
/// own account before it is written down. mpv gives up at the same moment
/// the deadline fires, and its reason — a network timeout, say — may excuse
/// the channel; see [`Machine::ledger`].
const FAILURE_SETTLE: Duration = Duration::from_secs(3);

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
    /// This open is being started at live: mpv was told to open paused, and
    /// the picture is held on its first frame until the connect burst is in.
    /// See [`Join`].
    joining: bool,
    /// The hold itself, from the first frame until it lets go.
    join: Option<Join>,
    /// Automatic catch-ups so far this session, for [`LiveJumpDto::seq`].
    jumps: u64,
    /// Whether this stream is live rather than a catch-up programme. A
    /// programme that plays to its end is finished; a live stream that ends
    /// was ended by somebody.
    live: bool,
    /// Set when this is a programme from a channel's archive, by
    /// `play_catchup` alone: what Live and an ending mean depend on it.
    catch_up: Option<CatchUp>,
    /// Whether this open's file has a video track, once its track list is
    /// known. A radio channel has none, and never shows a frame.
    has_video: Option<bool>,
    /// Whether this open's audio is playing.
    audio_on: bool,
    /// How this open was to meet the connect burst ([`Machine::starting`]),
    /// for an open that failed and then turned out to work.
    start_mode: LiveStart,
    /// Live was asked for during a catch-up programme of this channel, and
    /// the channel is being opened again: a second press is not a second
    /// open. Cleared when the open begins, or fails.
    reopening: Option<i64>,
    /// Whether a frame of this stream has been on screen: the difference
    /// between a stream that was taken away and one that never came.
    shown: bool,
    /// What mpv has logged about this stream; see `ipc::ending`.
    clues: Clues,
    /// When this open failed, for [`FAILURE_SETTLE`].
    failed_since: Option<Instant>,
    /// Whether this open's outcome — loaded, or failed — has gone to the
    /// channel's record. Once per open.
    noted: bool,
    /// When to read this open's quality for the channel's record; `None`
    /// once read, and for anything but a live channel that reached a picture.
    measure_at: Option<Instant>,
    /// When data last arrived and when its end was read, which tells a
    /// stalled connection from one the provider closed; see `ipc::ending`.
    flow: Flow,
    /// Whether the end of the data came too soon after too much of it for a
    /// broadcast: a placeholder clip, sent because the channel is off the
    /// air. See `ending::Placeholder`.
    placeholder: ending::Placeholder,
    /// This open was found off air and that has not yet gone to the
    /// channel's record.
    off_air_unnoted: bool,
    /// mpv has started a file since this open began (`start-file`): a
    /// picture after that is this stream's, even if the open was given up
    /// on first.
    started: bool,
    /// The watch for a placeholder is reading this open's own file: one
    /// has started since the open, and mpv's `path` is the address this
    /// open loaded. Until then mpv's cache state may still be the last
    /// file's — a movie or a programme near its end, a stream playing out
    /// its buffer — which must never be judged as this channel.
    armed: bool,
    /// When this open began, for how long the placeholder watch can still
    /// fire; see [`Machine::watch_settled`].
    began: Option<Instant>,
    /// A record the last open earned but had not yet written when it was
    /// replaced or stopped: written on the next heartbeat.
    carry: Option<Ledger>,
    /// Automatic reconnects made since a person last asked for this stream.
    reconnects: u32,
    /// When a reconnect last brought the picture back. A stream that has
    /// played a while since starts with a fresh allowance: a network that
    /// blips once an hour is not one that keeps failing.
    reconnected_at: Option<Instant>,
    /// A reconnect `ended_with` decided on, for the heartbeat to start.
    reconnect_due: Option<Due>,
    /// Whether the failure being reconnected from never reached the
    /// provider; see `connections::reconnect_verdict`.
    unreached: bool,
    /// The ending that started the reconnects, to show again if they are
    /// cancelled mid-attempt.
    chain_ending: Option<Ending>,
    /// Bumped by anything that makes a waiting reconnect stale: an open, a
    /// stop, Cancel. A reconnect checks it before acting.
    reconnect_token: u64,
}

/// What an open has shown about its channel that is worth keeping: see
/// `channel_quality` in `catalog::schema`. Only ever about live channels —
/// a catch-up programme that fails says nothing about the channel.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ledger {
    /// It reached a picture.
    Loaded(i64),
    /// It genuinely failed to load, for this reason.
    Failed(i64, String),
    /// It has played long enough to be measured.
    Measure(i64),
}

/// What the Live button (or `L`) does, for what is open: see
/// [`Machine::to_live`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToLive {
    /// Forward through the open stream, to its live edge.
    Seek,
    /// Open this channel again, live: what is open is a catch-up programme
    /// of it.
    Reopen(i64),
    /// Nothing: the channel is being opened again already.
    Already,
}

/// What kind of open this is: what Live, an ending and a reconnect mean
/// depend on it. Not what is opened, which is [`Opened`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenKind {
    /// A channel, live.
    Live,
    /// A programme from a channel's archive.
    CatchUp(CatchUp),
    /// A movie or an episode, on demand: neither live nor from an
    /// archive. Only ever with an on-demand [`Opened`]
    /// ([`Opened::is_on_demand`]).
    OnDemand,
}

/// A catch-up open, as `play_catchup` asked for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CatchUp {
    /// Asked for near the far end of the archive (within [`NEAR_EDGE`] of
    /// the earliest moment worth asking for), or of an archive of unknown
    /// depth: a refusal or an empty answer is then most likely the archive
    /// having let it go, rather than the account's connection limit.
    near_edge: bool,
}

/// How near the archive's far end a request counts as near it.
const NEAR_EDGE: i64 = 3600;

/// A reconnect to make, after a wait.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Due {
    delay: Duration,
    unreached: bool,
}

/// How long a reconnected stream plays before its allowance is restored.
const RECONNECT_FORGIVEN: Duration = Duration::from_secs(30);

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
            joining: false,
            join: None,
            jumps: 0,
            live: true,
            catch_up: None,
            has_video: None,
            audio_on: false,
            start_mode: LiveStart::CatchUp,
            reopening: None,
            shown: false,
            clues: Clues::default(),
            failed_since: None,
            noted: false,
            measure_at: None,
            flow: Flow::default(),
            placeholder: ending::Placeholder::default(),
            off_air_unnoted: false,
            started: false,
            armed: false,
            began: None,
            carry: None,
            reconnects: 0,
            reconnected_at: None,
            reconnect_due: None,
            unreached: false,
            chain_ending: None,
            reconnect_token: 0,
        }
    }

    /// What this moment adds to the channel's record, if anything. At most
    /// one thing per call: loading and measuring are seconds apart, and a
    /// failure excludes both.
    ///
    /// A failure is written once mpv has had its say: at once for anything
    /// mpv reported, after [`FAILURE_SETTLE`] for a bare deadline, whose
    /// reason may still arrive and may well excuse the channel.
    fn ledger(&mut self, now: Instant) -> Option<Ledger> {
        if let Some(carried) = self.carry.take() {
            return Some(carried);
        }
        let id = self.dto.stream_id?;
        if !self.live {
            return None;
        }
        // Off air: written whether or not the clip showed a frame. The
        // frame was never written as a load: see below.
        if self.off_air_unnoted {
            self.off_air_unnoted = false;
            self.noted = true;
            let why = ending::against_channel(&self.dto.ending?, None)?;
            return Some(Ledger::Failed(id, why));
        }
        // A picture, and a measurement of it, are held back while the open
        // could still turn out to be a placeholder clip: its first frame is
        // a frame, and its black 1080p a reading, of nothing. Written once
        // it can no longer, so that a load never lands after (and undoes)
        // an off-air record. See `could_be_off_air` for why a normal
        // channel is never held.
        if self.could_be_off_air(now) {
            return None;
        }
        if self.shown && !self.noted {
            self.noted = true;
            return Some(Ledger::Loaded(id));
        }
        if self.dto.state == PlaybackState::Failed && !self.shown && !self.noted {
            let bare_deadline = self
                .dto
                .ending
                .is_some_and(|e| e.kind == ending::EndingKind::NoVideo);
            let settled = self
                .failed_since
                .is_some_and(|t| now.saturating_duration_since(t) >= FAILURE_SETTLE);
            if bare_deadline && !settled {
                return None;
            }
            // Without an ending there is nothing to judge yet, and nothing
            // is noted: an open given up on without a word may still play.
            let e = self.dto.ending?;
            self.noted = true;
            let why = ending::against_channel(&e, self.dto.error.as_deref())?;
            return Some(Ledger::Failed(id, why));
        }
        if matches!(self.dto.state, PlaybackState::Playing | PlaybackState::Paused)
            && self.measure_at.is_some_and(|t| now >= t)
        {
            self.measure_at = None;
            return Some(Ledger::Measure(id));
        }
        None
    }

    /// A channel or a title is opening. A catch-up programme sets `live`
    /// false straight after, and so does a movie or an episode.
    fn opening(&mut self, what: Opened) {
        self.carry_unwritten_load();
        let title = what.is_on_demand();
        // Nothing about a title is "live", and nothing keeps it near an edge.
        self.dto.at_live_edge = !title;
        self.following_live = !title;
        self.last_live_nudge = None;
        self.drift_secs = 0.0;
        self.last_pos = None;
        self.last_tick_at = None;
        self.dto.state = PlaybackState::Opening;
        self.dto.stream_id = match what {
            Opened::Channel(id) => Some(id),
            Opened::Movie(_) | Opened::Episode(_) => None,
        };
        let on_demand = |kind, id| OnDemandDto {
            kind,
            id,
            position: 0.0,
            duration: None,
            buffered_to: None,
        };
        self.dto.on_demand = match what {
            Opened::Channel(_) => None,
            Opened::Movie(id) => Some(on_demand(OnDemandKind::Movie, id)),
            Opened::Episode(id) => Some(on_demand(OnDemandKind::Episode, id)),
        };
        self.dto.error = None;
        self.dto.ending = None;
        self.live = true;
        self.catch_up = None;
        self.has_video = None;
        self.audio_on = false;
        self.reopening = None;
        self.shown = false;
        self.clues = Clues::default();
        self.failed_since = None;
        self.noted = false;
        self.measure_at = None;
        self.flow = Flow::default();
        self.placeholder = ending::Placeholder::default();
        self.off_air_unnoted = false;
        self.started = false;
        self.armed = false;
        self.began = Some(Instant::now());
        // Whoever opens this decides whether it continues a run of
        // reconnects (see `open_as`); anything waiting is stale.
        self.reconnects = 0;
        self.reconnected_at = None;
        self.reconnect_due = None;
        self.reconnect_token += 1;
        self.dto.reconnect = None;
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
        // Asked for by the caller, after this, for a live channel.
        self.joining = false;
        self.join = None;
        self.dto.joining_live = false;
        // The last stream's catch-up is not news about this one.
        self.dto.live_jump = None;
    }

    /// How this open meets the connect burst, once `live` is set. A
    /// catch-up programme has no burst: it starts where it was asked to,
    /// whatever the setting.
    fn starting(&mut self, mode: LiveStart) {
        self.start_mode = mode;
        self.joining = self.live && mode == LiveStart::Wait;
        // Staying put means nothing skips ahead by itself — neither the
        // catch-up after the burst nor the one after a long stall. Asking
        // for live (`seek_live`) turns following back on.
        //
        // A catch-up programme never follows anything: the end of what it
        // has read is not live, and skipping toward it would skip the
        // programme. Nor is it at the live edge, whatever its buffer says,
        // so Live is offered from the start. A movie is not live either,
        // and has no edge to follow.
        self.following_live = self.live && mode != LiveStart::Stay;
        if !self.live {
            self.dto.at_live_edge = false;
        }
    }

    /// The viewer asked for live: stay at the edge from now on. A movie
    /// or an episode has none.
    fn follow_live(&mut self) -> Result<(), String> {
        if self.dto.on_demand.is_some() {
            return Err("a title has no live edge".into());
        }
        self.following_live = true;
        self.last_live_nudge = None;
        Ok(())
    }

    /// The live reopen of `stream_id` has begun ([`ToLive::Reopen`]).
    fn reopen_started(&mut self, stream_id: i64) {
        self.reopening = Some(stream_id);
    }

    /// The live reopen of `stream_id` could not be made. Said only if
    /// nothing has happened since: not over a newer channel, or a stop.
    fn reopen_failed(&mut self, stream_id: i64, why: &str) {
        if self.reopening == Some(stream_id) && self.dto.stream_id == Some(stream_id) {
            self.reopening = None;
            self.failed(why);
        }
    }

    /// What asking for live means for what is open now.
    fn to_live(&self) -> ToLive {
        if self.reopening.is_some() {
            return ToLive::Already;
        }
        match self.dto.stream_id {
            // A catch-up programme has no live edge to seek to: the channel
            // is opened again, live. Only a programme `play_catchup` opened:
            // a movie has no channel to go back to.
            Some(id) if self.catch_up.is_some() && self.dto.state != PlaybackState::Stopped => {
                ToLive::Reopen(id)
            }
            _ => ToLive::Seek,
        }
    }

    /// mpv reconfigured its video output, `with_video` when there is a
    /// picture behind it now. Returns whether it counted as a frame.
    ///
    /// Taking the last stream's picture down is a reconfigure too, and it
    /// can be pumped after the next open began: counted, it made an open
    /// that went on to be refused read as one that had played.
    fn reconfigured(&mut self, with_video: bool) -> bool {
        if with_video {
            self.playing();
        }
        with_video
    }

    /// The file's track list, when it has one: whether there is video in
    /// it ([`tracks::has_video`]).
    fn tracks_known(&mut self, has_video: Option<bool>) {
        if has_video.is_some() {
            self.has_video = has_video;
        }
        self.audio_only();
    }

    /// Whether this open's file has video is still to be learnt
    /// once it has started. mpv says nothing when a track list is the same
    /// as the last one — two stations from one provider, say — so then the
    /// heartbeat reads it.
    fn needs_track_list(&self) -> bool {
        self.started && self.has_video.is_none()
    }

    /// Whether audio is playing now (mpv has `audio-params`).
    fn audio(&mut self, on: bool) {
        self.audio_on = on;
        self.audio_only();
    }

    /// A radio channel never shows a frame, so its first sound is its
    /// "it works": once this open's file has started, its track list has no
    /// video in it, and audio is playing. Without this it failed on the
    /// first-frame deadline every time, with the sound playing behind the
    /// message, and was marked as a channel that does not load.
    fn audio_only(&mut self) {
        if self.started && self.audio_on && self.has_video == Some(false) && !self.shown {
            self.playing();
        }
    }

    /// One of mpv's log lines, kept as evidence while this open can still
    /// be explained: while it opens or plays, and after the first-frame
    /// deadline, whose bare "no video" mpv's reason may yet refine — a slow
    /// archive's 404 arrives after it.
    fn heard(&mut self, text: &str) {
        let open = matches!(
            self.dto.state,
            PlaybackState::Opening | PlaybackState::Playing | PlaybackState::Paused
        );
        if open || self.bare_deadline() {
            self.clues.observe(text);
        }
    }

    /// Failed on the first-frame deadline alone, with mpv still trying.
    fn bare_deadline(&self) -> bool {
        self.dto.state == PlaybackState::Failed
            && self.dto.ending.is_some_and(|e| e.kind == ending::EndingKind::NoVideo)
    }

    /// First frame decoded. The only honest "it works" edge: `file-loaded`
    /// fires on a stream that connects and then stalls.
    fn playing(&mut self) {
        if matches!(self.dto.state, PlaybackState::Stopped) || self.is_off_air() {
            return;
        }
        // Failed for a reason mpv gave, the stream is over: a reconfigure
        // now is the previous stream's picture being taken down, which a
        // refusal from a nearby server can beat. Only a bare deadline is
        // still loading, and its first frame is welcome; so is the first
        // frame of an open given up on without a word (the player did not
        // answer in time) whose file mpv has since started.
        if self.dto.state == PlaybackState::Failed {
            let revivable = match self.dto.ending {
                Some(_) => self.bare_deadline(),
                None => self.started,
            };
            if !revivable {
                return;
            }
            self.revive();
        }
        if self.joining {
            // Held on the first frame: there is a picture, but it is not
            // moving, so the stream is not playing yet. The deadline is
            // disarmed — the stream works — and the hold begins.
            if self.join.is_none() {
                self.came_back();
                self.shown = true;
                self.opened_at = None;
                self.dto.error = None;
                self.dto.ending = None;
                self.dto.joining_live = true;
                self.join = Some(Join::new());
            }
            return;
        }
        self.dto.state = if self.dto.paused_flag() {
            PlaybackState::Paused
        } else {
            PlaybackState::Playing
        };
        self.dto.error = None;
        self.dto.ending = None;
        self.came_back();
        self.shown = true;
        // Only on the way out of Opening: a later reconfigure (a resolution
        // change mid-stream) is not a new connection and has no burst, and
        // has already been measured.
        // A catch-up programme has no burst to catch up on: it starts where
        // it was asked to.
        if self.opened_at.is_some() && self.live {
            self.settle = Some(Settle::new());
            self.measure_at = Some(Instant::now() + MEASURE_AFTER);
        }
        self.opened_at = None;
    }

    /// Watches this open's data for a placeholder clip, `dt` seconds after
    /// the last heartbeat, and stops on finding one: the channel is off air.
    /// Returns the clip's length then, for the caller to stop mpv, which
    /// would otherwise skip to the clip's end (the hold and the catch-up
    /// after a burst both would) or play it out.
    ///
    /// Before anything else in the heartbeat moves the picture. Only for a
    /// live channel; see `ending::off_air`.
    /// `format` is mpv's `file-format`, read when the end is.
    fn watch_for_placeholder(&mut self, dt: f64, p: &Probe, format: Option<&str>) -> Option<f64> {
        if !self.armed
            || !matches!(
                self.dto.state,
                PlaybackState::Opening | PlaybackState::Playing | PlaybackState::Paused
            )
        {
            return None;
        }
        let read = self.placeholder.observe(
            dt,
            p.cache_start.or(p.time_pos),
            p.cache_end,
            p.demuxer_eof,
            p.cache_bytes,
        )?;
        let cx = ending::Context {
            live: self.live && self.dto.on_demand.is_none(),
            format,
            network: self.clues.network,
        };
        let clip = ending::off_air(read, cx)?;
        tracing::info!(
            "off air: the provider sent {:.0}s of stream and closed it {:.1}s after the open; \
             a placeholder, not the channel. Stopping",
            read.span,
            read.at
        );
        self.off_air(clip);
        Some(clip)
    }

    /// The channel is off air: the provider sent a placeholder clip `clip`
    /// seconds long instead of it. Stopped, said so, and written against
    /// the channel. Never reconnected: the provider would only send the
    /// clip again.
    fn off_air(&mut self, clip: f64) {
        self.dto.at_live_edge = true;
        self.following_live = false;
        self.settle = None;
        self.failed("the provider sent a placeholder instead of the channel");
        self.dto.ending = Some(Ending::off_air(clip));
        self.off_air_unnoted = true;
    }

    /// mpv started a file. The watch is armed once that file is known to be
    /// this open's: see [`Machine::arm`]. A radio channel may be playing
    /// from here: see [`Machine::audio_only`].
    fn file_started(&mut self) {
        self.started = true;
        self.audio_only();
    }

    /// Arms the placeholder watch if a file has started since this open
    /// began and `ours` — mpv's `path` is the address this open loaded.
    fn arm(&mut self, ours: bool) {
        if self.started && ours {
            self.armed = true;
        }
    }

    /// Whether this open could still turn out to be a placeholder clip: the
    /// watch is reading this open's file, its buffer is clip-shaped (at
    /// least [`ending::OFF_AIR_MIN_CLIP`] of stream already in), its end has
    /// not been judged, the window is not over, and the stream is open.
    ///
    /// Loads and measurements wait while it is. A normal live channel never
    /// has a buffer that deep that soon, so it is never held: its load is
    /// written at its first frame and its measurement after
    /// [`MEASURE_AFTER`], as before.
    fn could_be_off_air(&self, now: Instant) -> bool {
        self.armed
            && !self.placeholder.judged()
            && self.placeholder.span() >= ending::OFF_AIR_MIN_CLIP
            && matches!(
                self.dto.state,
                PlaybackState::Opening | PlaybackState::Playing | PlaybackState::Paused
            )
            && self
                .began
                .is_some_and(|t| now.saturating_duration_since(t).as_secs_f64() <= ending::OFF_AIR_WINDOW)
    }

    /// A live channel that showed a picture and is being replaced or
    /// stopped before its load was written: the load stands, so it is kept
    /// for the next heartbeat to write.
    fn carry_unwritten_load(&mut self) {
        if self.live && self.shown && !self.noted && !self.is_off_air() && self.dto.on_demand.is_none() {
            if let Some(id) = self.dto.stream_id {
                self.noted = true;
                self.carry = Some(Ledger::Loaded(id));
            }
        }
    }

    fn is_off_air(&self) -> bool {
        self.dto.ending.is_some_and(|e| e.kind == EndingKind::OffAir)
    }

    /// A failed open that turned out to work, put back as its first frame
    /// would have found it: opening, set up for the start it was asked for
    /// (held at live and let go by the heartbeat under "wait", caught up
    /// after its burst, measured), and with its failure no longer on its
    /// record, so that it is noted as loaded.
    fn revive(&mut self) {
        self.dto.state = PlaybackState::Opening;
        self.dto.error = None;
        self.dto.ending = None;
        self.failed_since = None;
        self.noted = false;
        self.opened_at = Some(Instant::now());
        self.starting(self.start_mode);
    }

    /// The hold on the first frame is over and the caller is starting
    /// playback, at live or as near it as the burst allowed. [`Settle`]
    /// still watches: a burst that outlasted the hold is caught up the old
    /// way, and says so.
    fn joined(&mut self) {
        self.joining = false;
        self.join = None;
        self.dto.joining_live = false;
        if self.dto.state == PlaybackState::Opening {
            self.dto.state = PlaybackState::Playing;
            self.settle = Some(Settle::new());
            // The held first frame already counted as the open, so the
            // measurement `playing` schedules on the way out of Opening never
            // happens on this path; it starts from here instead, once the
            // picture is moving.
            self.measure_at = Some(Instant::now() + MEASURE_AFTER);
        }
    }

    /// A picture, on an open that was a reconnect: it worked.
    fn came_back(&mut self) {
        if self.dto.reconnect.take().is_some() {
            self.reconnected_at = Some(Instant::now());
        }
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
        if self.dto.state != PlaybackState::Failed {
            self.failed_since = Some(Instant::now());
        }
        self.following_live = false;
        self.reconnect_due = None;
        self.dto.reconnect = None;
        self.joining = false;
        self.join = None;
        self.dto.joining_live = false;
        self.dto.state = PlaybackState::Failed;
        self.dto.error = Some(why.into());
        self.dto.ending = None;
        self.opened_at = None;
    }

    fn stopped(&mut self) {
        self.carry_unwritten_load();
        self.reopening = None;
        self.dto.at_live_edge = true;
        self.following_live = false;
        self.reconnects = 0;
        self.reconnected_at = None;
        self.reconnect_due = None;
        self.reconnect_token += 1;
        self.chain_ending = None;
        self.dto.reconnect = None;
        self.joining = false;
        self.join = None;
        self.dto.joining_live = false;
        self.dto.state = PlaybackState::Stopped;
        self.dto.stream_id = None;
        self.dto.on_demand = None;
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
        // A catch-up programme that failed still has live to go back to.
        self.dto.at_live_edge = self.live;
        self.following_live = false;
        // A movie is over only when it got to its end: an EOF anywhere else
        // is the provider dropping the connection, which must keep the
        // place to resume from and not mark it watched. A length of zero
        // says nothing of where the end is.
        let at_end = self.dto.on_demand.is_none_or(|o| {
            o.duration
                .is_some_and(|d| d > 0.0 && d - o.position <= MOVIE_END_SLACK_SECS)
        });
        if how == How::Eof && self.shown && !self.live && at_end {
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
        let mut clues = self.clues;
        clues.stalled = self.flow.stalled();
        let mut e = ending::classify(how, self.shown, clues);
        if let Some(c) = self.catch_up {
            e = ending::for_catchup(e, self.shown, c.near_edge);
        }
        self.dto.ending = Some(e);
        self.plan_reconnect(e, clues.unreached && !self.shown);
    }

    /// Decides whether to reconnect by itself, and when: only after the
    /// network failed, never after the provider ended or refused the stream
    /// (see `ending::worth_reconnecting`), and only while the allowance
    /// lasts. Whether a connection is free is asked when the wait is over.
    fn plan_reconnect(&mut self, e: Ending, unreached: bool) {
        self.reconnect_due = None;
        self.dto.reconnect = None;
        if !ending::worth_reconnecting(&e, self.live) {
            return;
        }
        let attempt = self.reconnects + 1;
        let Some(delay) = connections::backoff(attempt, connections::jitter()) else {
            return;
        };
        self.unreached = unreached;
        self.chain_ending.get_or_insert(e);
        self.reconnect_due = Some(Due { delay, unreached });
        self.dto.reconnect = Some(ReconnectDto {
            attempt,
            of: connections::BACKOFF.len() as u32,
            checking: false,
        });
    }

    /// The account could not be asked, so this attempt was not made. It
    /// counts, and the next waits its turn.
    fn reconnect_skipped(&mut self) {
        self.reconnects += 1;
        if let Some(e) = self.dto.ending {
            self.plan_reconnect(e, self.unreached);
        }
    }

    /// The account's connections are all in use elsewhere: stop, and say
    /// so. Reconnecting would take one from another device.
    fn in_use(&mut self, slots: connections::Slots) {
        self.dto.reconnect = None;
        self.reconnect_due = None;
        self.dto.error = Some("the account's connections are in use".to_owned());
        self.dto.ending = Some(Ending {
            kind: EndingKind::InUse,
            status: None,
            limit: true,
            slots: Some(slots),
            clip_secs: None,
        });
    }

    /// One stream allowed and no way to see who has it: stop reconnecting,
    /// and leave the network's explanation and Retry.
    fn reconnect_abandoned(&mut self) {
        self.dto.reconnect = None;
        self.reconnect_due = None;
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
        self.report_jump(by);
    }

    /// The viewer asked for live (the Live button, `L`) and playback went
    /// `by` seconds forward. Bookkeeping only, after the seek was made.
    ///
    /// Reported like an automatic catch-up, so the picture says how far it
    /// went whichever way the jump came about — but only a jump worth
    /// mentioning: under [`MIN_REPORTED_JUMP`] the picture barely moved, and
    /// "Jumped 0.4s to live" would be news about nothing. The nudge clock is
    /// left alone; `seek_live` clears it, so the automatic catch-up is free
    /// to keep the viewer there.
    fn jumped_to_live(&mut self, by: f64) {
        self.drift_secs = 0.0;
        if by >= MIN_REPORTED_JUMP {
            self.report_jump(by);
        }
    }

    fn report_jump(&mut self, by: f64) {
        self.jumps += 1;
        self.dto.live_jump = Some(LiveJumpDto {
            seconds: round_tenth(by),
            seq: self.jumps,
        });
    }

    /// Folds a main-thread measurement in. Quality figures are only trusted
    /// once something is actually on screen.
    fn apply(&mut self, p: &Probe) {
        let playing = matches!(
            self.dto.state,
            PlaybackState::Playing | PlaybackState::Paused
        );
        // The held first frame is a frame: its size and codec are known.
        // Where it sits against live is not news, though — it is about to
        // move, and a readout saying "24s behind" for the moment before it
        // does would be the very jump this hold exists to hide.
        if playing || self.join.is_some() {
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
        }
        if playing {
            if let Some(od) = self.dto.on_demand.as_mut() {
                // Whole seconds: a title's clock does not need to be
                // re-rendered for every tenth, and the saved place is whole
                // seconds too.
                if let Some(t) = p.time_pos {
                    od.position = t.max(0.0).round();
                }
                if p.duration.is_some() {
                    od.duration = p.duration.map(f64::round);
                }
                od.buffered_to = p.cache_end.map(f64::round);
                self.dto.behind_live = 0.0;
                self.dto.at_live_edge = false;
            } else {
                self.dto.behind_live = round_tenth(p.behind_live);
                // A catch-up programme is never live, however close the
                // playhead is to the end of what it has read.
                self.dto.at_live_edge = self.live
                    && live_edge_with_hysteresis(self.dto.at_live_edge, self.dto.behind_live);
            }
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

/// What the player has open.
///
/// A movie is not a stream id: every list, the watch-time ledger, a
/// channel's quality, favourites, Record and the handover to the recording
/// service are all about channels, and a movie's id is from another table
/// that may well hold the same number. Keeping the two apart here is what
/// stops a movie being mistaken for the channel with that id anywhere
/// downstream. An episode is no more a stream id than a movie is, and its
/// id is from a third table: no more a movie's either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opened {
    Channel(i64),
    Movie(i64),
    Episode(i64),
}

impl Opened {
    /// A title rather than a channel: a movie or an episode.
    fn is_on_demand(self) -> bool {
        matches!(self, Opened::Movie(_) | Opened::Episode(_))
    }
}

/// The title an on-demand open is, for writing down how far into it the
/// viewer got.
fn item_of(o: &OnDemandDto) -> VodItem {
    match o.kind {
        OnDemandKind::Movie => VodItem::movie(o.id),
        OnDemandKind::Episode => VodItem::episode(o.id),
    }
}

/// How close to a movie's length its position has to be for an end of file
/// to be its end rather than a dropped connection.
const MOVIE_END_SLACK_SECS: f64 = 10.0;

static STATE: LazyLock<Mutex<Machine>> = LazyLock::new(|| Mutex::new(Machine::new()));

/// Watch time earned by whatever is on, for auto-favourites.
///
/// Driven from the heartbeat below, which is the only place that knows the
/// difference between a stream that is playing and one that is merely open.
/// The policy — what counts, and when it is worth a write — is
/// [`crate::catalog::activity::Accumulator`], and is pure.
static WATCHED: LazyLock<Mutex<Accumulator>> = LazyLock::new(|| Mutex::new(Accumulator::new()));

/// How far into a movie or an episode the viewer is, for resuming and for
/// "watched".
/// Driven from the heartbeat like [`WATCHED`]; the policy is
/// [`ProgressSaver`]'s.
static PROGRESS: LazyLock<Mutex<ProgressSaver>> = LazyLock::new(|| Mutex::new(ProgressSaver::new()));

/// The clock in seconds, with its fraction: the saver counts play from the
/// gaps between heartbeats, which whole seconds would quantise.
fn clock_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// Writes a title's place, off the main thread and for the same reasons as
/// [`persist`].
fn persist_progress(w: ProgressWrite) {
    tauri::async_runtime::spawn_blocking(move || {
        let r = crate::catalog::db::with_db(|c| write_progress(c, w, activity::now_secs()).map(|_| ()));
        if let Err(e) = r {
            tracing::warn!("could not record how far into a title playback got: {e}");
        }
    });
}

/// Writes the place `saver` holds that is not written yet, if any.
fn flush_progress(
    saver: &mut ProgressSaver,
    c: &rusqlite::Connection,
) -> anyhow::Result<Option<vod_store::Marks>> {
    saver
        .flush()
        .map(|w| write_progress(c, w, activity::now_secs()))
        .transpose()
}

/// Where a movie or an episode got to, written before the app quits.
///
/// The part of the stop command that matters at quit: its other part, giving
/// the recording service's connection back, is not needed, since the service
/// sees the app's connection close. Written here and now rather than on a
/// worker thread, because the process is about to end and a write queued for
/// later might not happen. Main thread, from the quit.
pub fn save_progress_now() {
    let Ok(mut saver) = PROGRESS.lock() else {
        return;
    };
    if let Err(e) = crate::catalog::db::with_db(|c| flush_progress(&mut saver, c)) {
        tracing::warn!("could not record how far into a title playback got: {e}");
    }
}

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

/// Writes what an open showed about its channel, off the main thread, and
/// tells the frontend the channel's record as it now stands, so the list
/// dims or un-dims the row and its badge says what was just measured
/// without waiting for its page to be fetched again. A failure to write is
/// logged and dropped, as watch time's is.
fn remember(app: &AppHandle, ledger: Ledger, reading: Option<store::Reading>) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let at = activity::now_secs();
        let r = crate::catalog::db::with_db(|c| {
            let id = match &ledger {
                Ledger::Loaded(id) => {
                    store::record_loaded(c, *id)?;
                    *id
                }
                Ledger::Failed(id, why) => {
                    store::record_failed(c, *id, why, at)?;
                    *id
                }
                Ledger::Measure(id) => {
                    if let Some(r) = &reading {
                        store::record_reading(c, *id, r, at)?;
                    }
                    *id
                }
            };
            let quality = store::channels_by_ids(c, &[id])?
                .into_iter()
                .next()
                .and_then(|ch| ch.quality);
            Ok(ChannelQualityDto {
                stream_id: id,
                quality,
            })
        });
        match r {
            Ok(news) => {
                let _ = app.emit(events::CHANNEL_QUALITY, news);
            }
            Err(e) => tracing::warn!("could not record what playing a channel showed: {e}"),
        }
    });
}

/// Mutates the state and emits `PLAYER_STATE` if anything visible changed.
fn commit(app: &AppHandle, f: impl FnOnce(&mut Machine)) {
    let (payload, holding) = {
        let mut m = STATE.lock().expect("player state mutex poisoned");
        f(&mut m);
        (m.changed(), holds_a_stream(&m))
    };
    connections::set_held(connections::Use::Player, holding);
    if let Some(p) = payload {
        let _ = app.emit(events::PLAYER_STATE, p);
    }
}

/// Whether the player has a stream open, as the connection budget counts.
/// Through the recording service it holds none of its own: the service
/// does, and counts it.
fn holds_a_stream(m: &Machine) -> bool {
    !THROUGH_SERVICE.load(Ordering::Acquire)
        && matches!(
            m.dto.state,
            PlaybackState::Opening | PlaybackState::Playing | PlaybackState::Paused
        )
}

// ---------------------------------------------------------------------------
// Where the stream comes from
// ---------------------------------------------------------------------------

/// Where the player reads a channel from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Via {
    /// The provider, on a connection of the player's own.
    Direct,
    /// The recording service on this computer: a place (`viewer`) on its
    /// connection to the channel, which a recording rides too
    /// (`recording::handover`). Read with the app's token, sent as a header
    /// (`header`) so it is never part of an address mpv logs.
    Service {
        viewer: String,
        header: Option<String>,
    },
}

/// Whether the stream open now comes through the recording service.
static THROUGH_SERVICE: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Reconnecting
// ---------------------------------------------------------------------------

/// What was last opened, so that a reconnect can open it again. Kept here,
/// in memory only: the URL carries the account.
static LAST_OPEN: Mutex<Option<(Opened, Playable, OpenKind, Via)>> = Mutex::new(None);

/// Whether a reconnect decided under `token` is still wanted.
fn still_wanted(token: u64) -> bool {
    STATE
        .lock()
        .map(|m| m.reconnect_token == token)
        .unwrap_or(false)
}

/// Waits out a reconnect's back-off, asks the account whether a connection
/// is free, and reconnects only if one is.
///
/// Off the main thread, and abandoned the moment anything else happens to
/// the player: a zap, a stop, Cancel.
fn spawn_reconnect(app: &AppHandle, due: Due, token: u64, stream_id: i64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(due.delay).await;
        if !still_wanted(token) {
            return;
        }
        // Through the recording service: a place on its connection again,
        // which costs the account nothing, while it still has one. Its old
        // place may be gone (a viewer nobody reads is let go).
        let through_service = LAST_OPEN
            .lock()
            .ok()
            .and_then(|l| l.clone())
            .filter(|(id, ..)| *id == Opened::Channel(stream_id))
            .and_then(|(_, _, kind, via)| match via {
                Via::Service { .. } => Some(kind),
                Via::Direct => None,
            });
        if let Some(kind) = through_service {
            if let Some((target, via)) = through_service_target(stream_id).await {
                if !still_wanted(token) {
                    return;
                }
                let attempt = STATE
                    .lock()
                    .map(|m| m.reconnects + 1)
                    .unwrap_or(1);
                if let Err(e) = open_as(&app, Opened::Channel(stream_id), target, kind, Some(attempt), via, None, None).await {
                    tracing::warn!("reconnect could not be sent: {e}");
                }
                return;
            }
            // The service no longer has the channel: the player's own
            // connection, if the account has one to spare (below).
            match direct_target(stream_id) {
                Ok(t) => {
                    if let Ok(mut last) = LAST_OPEN.lock() {
                        *last = Some((Opened::Channel(stream_id), t, kind, Via::Direct));
                    }
                }
                Err(e) => {
                    tracing::warn!("reconnect: {e}");
                    commit(&app, |m| {
                        if m.reconnect_token == token {
                            m.reconnect_abandoned();
                        }
                    });
                    return;
                }
            }
        }
        commit(&app, |m| {
            if let Some(r) = m.dto.reconnect.as_mut() {
                r.checking = true;
            }
        });
        let mut verdict = connections::reconnect_verdict(limit_now().await, due.unreached, false);
        if verdict == Verdict::LookAgain {
            // The first look may still count the connection that just died.
            tokio::time::sleep(connections::SECOND_LOOK).await;
            if !still_wanted(token) {
                return;
            }
            verdict = connections::reconnect_verdict(limit_now().await, due.unreached, true);
        }
        tracing::info!("network failure; reconnect verdict {verdict:?}");
        let target = LAST_OPEN
            .lock()
            .ok()
            .and_then(|l| l.clone())
            .filter(|(id, ..)| *id == Opened::Channel(stream_id));
        match (verdict, target) {
            (Verdict::Go, Some((id, target, kind, via))) => {
                let attempt = {
                    let m = STATE.lock().expect("player state mutex poisoned");
                    if m.reconnect_token != token {
                        return;
                    }
                    m.reconnects + 1
                };
                if let Err(e) = open_as(&app, id, target, kind, Some(attempt), via, None, None).await {
                    tracing::warn!("reconnect could not be sent: {e}");
                }
            }
            (Verdict::NotYet, _) => commit(&app, |m| {
                if m.reconnect_token == token {
                    m.reconnect_skipped();
                }
            }),
            (Verdict::InUse(slots), _) => commit(&app, |m| {
                if m.reconnect_token == token {
                    m.in_use(slots);
                }
            }),
            _ => commit(&app, |m| {
                if m.reconnect_token == token {
                    m.reconnect_abandoned();
                }
            }),
        }
    });
}

/// The account's connections as they stand, for a reconnect.
async fn limit_now() -> Limit {
    let declared = crate::catalog::settings::load_or_default().simultaneous_streams;
    match source() {
        Ok(src) => connections::look(src, declared).await.limit(),
        Err(_) => Limit::Unknown,
    }
}

/// Stops reconnecting. Mid-attempt, the attempt is stopped too, and the
/// failure that started it is shown again with Retry.
#[tauri::command]
pub async fn cancel_reconnect(app: AppHandle) -> Result<(), String> {
    let attempting = {
        let mut m = STATE.lock().expect("player state mutex poisoned");
        let attempting = m.dto.reconnect.is_some() && m.dto.state == PlaybackState::Opening;
        m.reconnect_token += 1;
        attempting
    };
    if attempting {
        on_player(&app, |p| p.command("stop", &[]).map_err(|e| e.to_string())).await?;
    }
    commit(&app, |m| {
        let ending = m.chain_ending.take().or(m.dto.ending);
        if attempting {
            m.failed("reconnecting was cancelled");
        }
        m.reconnect_abandoned();
        m.reconnects = 0;
        m.dto.ending = ending;
    });
    Ok(())
}

/// The account's connections, for Settings → Account: how many it allows,
/// how many are open, when it expires.
#[tauri::command]
pub async fn connection_status() -> Result<ConnectionsDto, String> {
    let declared_max = crate::catalog::settings::load_or_default().simultaneous_streams;
    let src = source()?;
    let declared = matches!(src, Source::M3u(_) | Source::Tvheadend(_));
    // An account that cannot be asked has nothing worth keeping; one that
    // can is asked at most every few seconds.
    let s = if declared {
        connections::look(src, declared_max).await
    } else {
        connections::look_cached(src, declared_max).await
    };
    Ok(ConnectionsDto {
        max: s.max,
        active: s.active,
        ours: connections::held(),
        counted: matches!(s.limit(), Limit::Counted(_)),
        declared,
        expires: s.expires,
    })
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

/// A heartbeat's signals in the order they happened, as near as can be told.
///
/// mpv hands out every queued event before any log line waiting beside it
/// (`mpv_wait_event` drains its event queue first), so when a stream is
/// refused between two heartbeats — at once, by a server nearby — its end
/// comes out of the pump before the line that says why, and the line would
/// arrive with the stream already failed and unexplained. The lines were
/// logged first, so they are read first.
fn logs_first(signals: &[Signal]) -> Vec<&Signal> {
    let (logs, events): (Vec<&Signal>, Vec<&Signal>) =
        signals.iter().partition(|s| matches!(s, Signal::Log(_)));
    logs.into_iter().chain(events).collect()
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
    let audio_on = p.get_string("audio-params/format").is_some();
    // Assigned inside the block below, where the state lock is held.
    let earned;
    let loaded;
    let newly_failed;
    let progress;
    let ledger;
    let due;
    let holding;
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
    let mut finished = None;
    // Whether the file mpv has is the one the current open loaded: its
    // `path` against the address. Read before the state lock, and kept out
    // of every log: the address can carry the account.
    let this_opens_file = {
        let loaded = LAST_OPEN
            .lock()
            .ok()
            .and_then(|l| l.as_ref().map(|(_, target, _, _)| target.url.clone()));
        loaded.is_some_and(|url| p.get_string("path").is_some_and(|path| path == url))
    };
    let payload = {
        let mut m = STATE.lock().expect("player state mutex poisoned");
        let was_failed = m.dto.state == PlaybackState::Failed;
        for s in logs_first(&signals) {
            match s {
                Signal::VideoReconfig => {
                    if !m.reconfigured(probe.width.is_some()) {
                        continue;
                    }
                    // Which decoder is doing the work, once per reconfigure.
                    // Software decoding is the difference between a busy CPU
                    // being unnoticeable and it dropping frames and starving
                    // the audio, and it falls back silently — so it is said.
                    tracing::info!(
                        "decoding {} with {}",
                        p.get_string("video-codec").unwrap_or_else(|| "video".into()),
                        decoder_words(p.get_string("hwdec-current").as_deref())
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
                        let title = m.dto.on_demand.filter(|_| m.shown);
                        m.ended();
                        // Played to its end: where it got to is the whole of
                        // it, which is what marks it watched.
                        if m.dto.state == PlaybackState::Stopped {
                            finished = title;
                        }
                    }
                }
                // Evidence for why it might end; see `ipc::ending` and
                // `Machine::heard`.
                Signal::Log(text) => m.heard(text),
                Signal::StartFile => m.file_started(),
                Signal::FileLoaded | Signal::Tracks(_) => {}
            }
        }
        // A radio channel's "it works"; see `Machine::audio_only`.
        let listed = match tracks {
            Some(json) => tracks::has_video(json),
            None if m.needs_track_list() => p.get_string("track-list").as_deref().and_then(tracks::has_video),
            None => None,
        };
        m.tracks_known(listed);
        m.audio(audio_on);
        m.apply(&probe);
        m.check_deadline();
        // Wall time since the last heartbeat, which is never exactly TICK.
        let elapsed = m
            .last_tick_at
            .map_or(TICK.as_secs_f64(), |t| t.elapsed().as_secs_f64());
        // A placeholder clip rather than the channel: stopped here, before
        // the hold or the catch-up below can skip to the clip's end.
        let holding_first_frame = m.join.is_some();
        m.arm(this_opens_file);
        // Which demuxer read it, asked only when there is an end to judge.
        let format = if probe.demuxer_eof { p.get_string("file-format") } else { None };
        if m.watch_for_placeholder(elapsed, &probe, format.as_deref()).is_some() {
            if let Err(e) = p.command("stop", &[]) {
                tracing::warn!("could not stop the placeholder: {e}");
            }
            // `pause` outlives the file: the hold's is let go of here, as
            // nothing else will.
            if holding_first_frame {
                if let Err(e) = p.set_flag("pause", false) {
                    tracing::warn!("could not let go of the held frame: {e}");
                }
            }
        }
        ledger = m.ledger(Instant::now());

        // Starting at live: the picture is held on its first frame until the
        // connect burst is in, then started at the cushion. See `Join`.
        if m.join.is_some() {
            if !probe.paused {
                // Somebody pressed play during the hold. That is an answer
                // too: play from here, and `Settle` catches it up after.
                m.joined();
            } else if let Some(by) = m
                .join
                .as_mut()
                .and_then(|j| j.observe(elapsed, probe.cache_end, probe.behind_live))
            {
                let burst = m.join.as_ref().is_some_and(Join::saw_burst);
                tracing::info!(
                    "starting at live: {:.1}s of data in, {}; skipping {by:.1}s of it",
                    probe.behind_live,
                    if burst { "the connect burst is over" } else { "no connect burst" },
                );
                if by > 0.05 {
                    // Paused, so this is unseen: the first frame that moves
                    // is the one it lands on.
                    let _ = p.command("seek", &[&format!("{by:.3}"), "relative"]);
                }
                if let Err(e) = p.set_flag("pause", false) {
                    tracing::warn!("could not start the held channel: {e}");
                }
                m.joined();
            }
        }
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

        // Evidence for telling a stall from a stream the provider closed.
        if matches!(
            m.dto.state,
            PlaybackState::Opening | PlaybackState::Playing | PlaybackState::Paused
        ) {
            m.flow.observe(elapsed, probe.cache_end, probe.demuxer_eof, probe.fw_bytes);
        }
        if m.dto.state == PlaybackState::Playing
            && m.reconnected_at.is_some_and(|t| t.elapsed() >= RECONNECT_FORGIVEN)
        {
            m.reconnects = 0;
            m.reconnected_at = None;
            m.chain_ending = None;
        }
        due = m
            .reconnect_due
            .take()
            .zip(m.dto.stream_id)
            .map(|(d, id)| (d, m.reconnect_token, id));
        holding = holds_a_stream(&m);

        // Watch time, for auto-favourites. Only `Playing` counts: opening is
        // not watching, and neither is a paused stream sitting on a frame.
        earned = WATCHED.lock().expect("activity mutex poisoned").observe(
            m.dto.stream_id,
            m.dto.state == PlaybackState::Playing,
            TICK.as_secs_f64(),
            activity::now_secs(),
        );

        // How far into a title, for resuming. Nothing is reported before a
        // picture is up: the position a title opens at is not where the
        // viewer got to.
        progress = {
            let mut saver = PROGRESS.lock().expect("progress mutex poisoned");
            match finished {
                Some(od) => {
                    let end = od.duration.unwrap_or(od.position);
                    saver
                        .observe(Some(item_of(&od)), Some(end), od.duration, false, clock_secs())
                        .or_else(|| saver.flush())
                }
                None => {
                    let shown = matches!(m.dto.state, PlaybackState::Playing | PlaybackState::Paused);
                    let od = m.dto.on_demand;
                    saver.observe(
                        od.as_ref().map(item_of),
                        od.filter(|_| shown).map(|o| o.position),
                        od.and_then(|o| o.duration),
                        m.dto.state == PlaybackState::Playing,
                        clock_secs(),
                    )
                }
            }
        };

        // Nothing loaded is not a reading of zero buffer, it is no reading:
        // recording it would fill the graph with a flat line drawn while
        // there was nothing to draw.
        loaded = m.dto.stream_id.is_some() || m.dto.on_demand.is_some();

        m.changed()
    };
    if loaded {
        crate::ipc::health::observe(health);
    }
    connections::set_held(connections::Use::Player, holding);
    if let Some((d, token, id)) = due {
        spawn_reconnect(app, d, token, id);
    }
    if let Some(id) = newly_failed {
        explain_failure(app, id);
    }
    if let Some(e) = earned {
        persist(e);
    }
    if let Some(w) = progress {
        persist_progress(w);
    }
    if let Some(l) = ledger {
        // Read here, on the main thread where the player lives; written
        // elsewhere, like the watch time.
        let reading = matches!(l, Ledger::Measure(_)).then(|| p.reading());
        remember(app, l, reading);
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
    let cached = CLIENT.read().ok().and_then(|slot| slot.map(|(s, _)| s));
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
///
/// Kept beside the account file's [`credentials::fingerprint`] as it was when
/// the source was built. The app is not the only writer of that file: the
/// recording service on this computer saves it too, when a browser signed in
/// to it changes the account, and nothing tells the app. So [`source`] looks
/// at the file each time and builds again when it has changed.
type Cached = (&'static Source, Option<u64>);
static CLIENT: std::sync::RwLock<Option<Cached>> = std::sync::RwLock::new(None);

/// Drops the cached client so the next call builds one from whatever is on
/// file now. Called when the account changes.
pub fn forget_source() {
    if let Ok(mut slot) = CLIENT.write() {
        *slot = None;
    }
    connections::forget();
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
    // Before the load, so a save landing between the two is seen next time
    // rather than missed.
    let now_on_file = credentials::fingerprint();
    if let Ok(slot) = CLIENT.read() {
        match *slot {
            Some((c, built_from)) if built_from == now_on_file => return Ok(c),
            Some(_) => {
                tracing::info!("the account file changed; using the account now on file");
                // What was known of the old account's connections is not
                // this one's.
                connections::forget();
            }
            None => {}
        }
    }
    let built = Source::load().map_err(|e| crate::catalog::sync_engine::scrub(&e.to_string()))?;
    // Leaked on purpose: the client lives for the process, and the few bytes
    // a replaced one costs are not worth threading a lifetime through every
    // caller. An account can only be changed a handful of times by hand.
    let leaked: &'static Source = Box::leak(Box::new(built));
    if let Ok(mut slot) = CLIENT.write() {
        *slot = Some((leaked, now_on_file));
    }
    Ok(leaked)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn play(app: AppHandle, stream_id: i64) -> Result<(), String> {
    play_channel(&app, stream_id).await
}

/// A channel, live, the way picking it does: see [`play`].
async fn play_channel(app: &AppHandle, stream_id: i64) -> Result<(), String> {
    // A channel the recording service on this computer is recording: a
    // place on its connection, which costs the account nothing, rather than
    // a second connection to the same channel.
    if crate::ipc::handover::captured(stream_id) {
        if let Some((target, via)) = through_service_target(stream_id).await {
            return open_as(app, Opened::Channel(stream_id), target, OpenKind::Live, None, via, None, None).await;
        }
    }
    play_direct(app, stream_id).await
}

/// The channel on the player's own connection to the provider.
async fn play_direct(app: &AppHandle, stream_id: i64) -> Result<(), String> {
    let target = direct_target(stream_id)?;
    open(app, stream_id, target, OpenKind::Live).await
}

/// Where the provider serves a live channel.
fn direct_target(stream_id: i64) -> Result<Playable, String> {
    Ok(match fixture_url(stream_id) {
        Some(u) => Playable::plain(u),
        None => source()?
            .live(stream_id)
            .map_err(|e| crate::catalog::sync_engine::scrub(&e.to_string()))?,
    })
}

/// Where the provider serves a movie.
///
/// Under a harness (`ZAPPER_FIXTURE_BASE`) it is a file on the fixture
/// server, `ZAPPER_FIXTURE_MOVIE` of it (default `movie.mp4`).
fn movie_target(movie_id: i64) -> Result<Playable, String> {
    if let Ok(base) = std::env::var("ZAPPER_FIXTURE_BASE") {
        let name = std::env::var("ZAPPER_FIXTURE_MOVIE").unwrap_or_else(|_| "movie.mp4".into());
        return Ok(Playable::plain(format!("{}/{name}", base.trim_end_matches('/'))));
    }
    source()?
        .movie(movie_id)
        .map_err(|e| crate::catalog::sync_engine::scrub(&e.to_string()))
}

/// Plays a movie, from `start` seconds in when given. On the player's own
/// connection, like a live channel and under the same budget: never through
/// the recording service, which has nothing to do with a title.
#[tauri::command]
pub async fn play_movie(app: AppHandle, movie_id: i64, start: Option<f64>) -> Result<(), String> {
    let target = movie_target(movie_id)?;
    open_as(&app, Opened::Movie(movie_id), target, OpenKind::OnDemand, None, Via::Direct, None, start).await
}

/// Where the provider serves an episode.
///
/// Under a harness (`ZAPPER_FIXTURE_BASE`) it is a file on the fixture
/// server, `ZAPPER_FIXTURE_EPISODE` of it (default `movie.mkv`).
fn episode_target(episode_id: i64) -> Result<Playable, String> {
    if let Ok(base) = std::env::var("ZAPPER_FIXTURE_BASE") {
        let name = std::env::var("ZAPPER_FIXTURE_EPISODE").unwrap_or_else(|_| "movie.mkv".into());
        return Ok(Playable::plain(format!("{}/{name}", base.trim_end_matches('/'))));
    }
    source()?
        .episode(episode_id)
        .map_err(|e| crate::catalog::sync_engine::scrub(&e.to_string()))
}

/// Plays an episode, from `start` seconds in when given: on the player's
/// own connection and under the same rules as a movie.
#[tauri::command]
pub async fn play_episode(app: AppHandle, episode_id: i64, start: Option<f64>) -> Result<(), String> {
    let target = episode_target(episode_id)?;
    open_as(&app, Opened::Episode(episode_id), target, OpenKind::OnDemand, None, Via::Direct, None, start).await
}

/// The player's current place on the recording service's connection, if
/// it has one.
fn current_viewer() -> Option<String> {
    LAST_OPEN.lock().ok().and_then(|l| match l.as_ref() {
        Some((_, _, _, Via::Service { viewer, .. })) => Some(viewer.clone()),
        _ => None,
    })
}

/// A place on a connection the recording service already has to the
/// channel, or `None` when it has none (or does not answer). Never a new
/// connection: that is the player's own to open.
async fn through_service_target(stream_id: i64) -> Option<(Playable, Via)> {
    let client = crate::recording::client::Client::local().ok()?;
    let req = PlayerWatch {
        stream_id,
        only_shared: true,
        replaces: current_viewer(),
        player: true,
        ..PlayerWatch::default()
    };
    match client.watch(&req).await {
        Ok(w) => Some(service_target(&client, &w)),
        Err(r) => {
            tracing::info!("not through the recording service: {}", r.error);
            None
        }
    }
}

fn service_target(client: &Client, w: &Watching) -> (Playable, Via) {
    let (url, header) = client.stream(w);
    (
        Playable::plain(url),
        Via::Service {
            viewer: w.viewer.clone(),
            header,
        },
    )
}

/// What the player shows now, for Record (`ipc::handover`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NowPlaying {
    pub stream_id: i64,
    /// A live channel, not a catch-up programme.
    pub live: bool,
    /// Through the recording service's connection.
    pub through_service: bool,
}

/// The channel open in the player, if one is.
pub fn now_playing() -> Option<NowPlaying> {
    let open = STATE.lock().ok().and_then(|m| {
        matches!(
            m.dto.state,
            PlaybackState::Opening | PlaybackState::Playing | PlaybackState::Paused
        )
        .then_some(m.dto.stream_id)
        .flatten()
    })?;
    let last = LAST_OPEN.lock().ok()?.clone()?;
    (last.0 == Opened::Channel(open)).then_some(NowPlaying {
        stream_id: open,
        live: last.2 == OpenKind::Live,
        through_service: matches!(last.3, Via::Service { .. }),
    })
}

/// Moves the player, which shows `stream_id` live on its own connection,
/// onto the recording service's connection to it (`recording::handover`):
/// its connection is let go first, so the service can have it on an account
/// that allows one, then the service is asked for a place, connecting as
/// the player would. The picture stops for the moment that takes; the
/// volume and the tracks chosen are kept. Should the service not manage it,
/// the player connects again by itself and says why.
pub async fn move_to_service(app: &AppHandle, client: &Client, stream_id: i64) -> Result<(), String> {
    let tracks = on_player(app, |p| {
        Ok((
            p.get_string("aid").unwrap_or_else(|| "auto".into()),
            p.get_string("sid").unwrap_or_else(|| "auto".into()),
        ))
    })
    .await
    .ok();
    commit(app, |m| {
        m.opening(Opened::Channel(stream_id));
        m.starting(live_start_mode());
    });
    on_player(app, |p| p.command("stop", &[]).map_err(|e| e.to_string())).await?;
    let req = PlayerWatch {
        stream_id,
        anyway: true,
        player: true,
        ..PlayerWatch::default()
    };
    let mut tries = 0;
    let answer = loop {
        // The first-frame deadline is the open's, not this wait's.
        commit(app, |m| m.opened_at = Some(Instant::now()));
        match client.watch(&req).await {
            Ok(w) => break Ok(w),
            // A provider slow to stop counting the player's connection
            // refuses the service's for a moment.
            Err(r) if tries < 2 && (r.limit || r.reason == "feed" || r.reason == "inUse") => {
                tries += 1;
                tracing::info!("the recording service could not connect yet: {}", r.error);
                tokio::time::sleep(connections::SECOND_LOOK).await;
            }
            Err(r) => break Err(r),
        }
    };
    match answer {
        Ok(w) => {
            let (target, via) = service_target(client, &w);
            open_as(app, Opened::Channel(stream_id), target, OpenKind::Live, None, via, tracks, None).await
        }
        Err(r) => {
            tracing::warn!("the player could not move onto the recording service: {}", r.error);
            if let Err(e) = play_direct(app, stream_id).await {
                tracing::warn!("and could not connect again by itself: {e}");
            }
            Err(r.error)
        }
    }
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
    let (start, duration_minutes, near_edge) = within_archive(stream_id, start, duration_minutes);
    let target = match source()? {
        Source::Xtream(server) => {
            // The server reads the timestamp as wall time in its own time
            // zone, which need not be this computer's: see
            // `catalog::panel_clock`.
            let offset = crate::catalog::panel_clock::offset_for(server.credentials(), start);
            let stamp = timeshift_stamp(start, offset);
            Playable::plain(server.timeshift_url(stream_id, &stamp, duration_minutes))
        }
        // A playlist's archive is whatever its entry's template says; the
        // local offset is there for templates written in wall-clock time.
        // `{utc}`, `{lutc}` and the rest are Unix time and need no zone. A
        // playlist has no account call that says which zone its server's
        // clock is in, so the wall-clock fields stay in this computer's,
        // as IPTV Simple writes them — `xc` entries included.
        Source::M3u(m) => m
            .catchup(
                stream_id,
                start,
                duration_minutes,
                activity::now_secs(),
                crate::localtime::utc_offset(start),
            )
            .map_err(|e| crate::catalog::sync_engine::scrub(&e.to_string()))?,
        // A tuner has no archive, and a Tvheadend server's is its DVR, which
        // is not read. Their channels are never marked as having catch-up,
        // so this is only reached by a stale request from before a switch.
        Source::HdHomeRun(_) | Source::Tvheadend(_) => {
            return Err("this account has no catch-up".to_owned());
        }
    };
    open(&app, stream_id, target, OpenKind::CatchUp(CatchUp { near_edge })).await
}

/// A catch-up request moved inside the channel's archive, by
/// [`clamp_to_archive`], when its depth is known; and whether it is near
/// the archive's far end ([`near_edge`]).
fn within_archive(stream_id: i64, start: i64, minutes: u32) -> (i64, u32, bool) {
    let days: Option<i64> = crate::catalog::db::with_db(|conn| {
        Ok(conn
            .query_row(
                "SELECT archive_days FROM channels WHERE stream_id = ?1",
                [stream_id],
                |r| r.get(0),
            )
            .ok()
            .flatten())
    })
    .ok()
    .flatten();
    let earliest = days
        .filter(|d| *d > 0)
        .map(|d| crate::catalog::guide_search::earliest_catchup(activity::now_secs(), d));
    let (start, minutes) = clamp_to_archive(start, minutes, earliest);
    (start, minutes, near_edge(start, earliest))
}

/// Whether a request starting at `start` is near the far end of an archive
/// whose earliest moment worth asking for is `earliest`: within
/// [`NEAR_EDGE`] of it, or anywhere when the archive's depth is not known.
fn near_edge(start: i64, earliest: Option<i64>) -> bool {
    earliest.is_none_or(|e| start < e + NEAR_EDGE)
}

/// Moves a request that starts before `earliest` up to it (to the next
/// whole minute, the server's resolution), keeping where it ends. A request
/// for footage the server has dropped fails outright; starting a little
/// later plays.
fn clamp_to_archive(start: i64, minutes: u32, earliest: Option<i64>) -> (i64, u32) {
    let Some(earliest) = earliest.filter(|e| start < *e) else {
        return (start, minutes);
    };
    let end = start + i64::from(minutes) * 60;
    let from = earliest.div_euclid(60) * 60 + if earliest.rem_euclid(60) == 0 { 0 } else { 60 };
    let left = (end - from).max(60);
    (from, u32::try_from((left + 59) / 60).unwrap_or(1))
}

/// Whether the last stream opened set its own HTTP headers. See
/// [`apply_http_headers`].
static CUSTOM_HEADERS: AtomicBool = AtomicBool::new(false);

/// Whether the last stream opened sent the recording service's token.
static SERVICE_HEADER: AtomicBool = AtomicBool::new(false);

/// Hands a stream's user agent and referrer to mpv before it is opened, and
/// the recording service's token to a stream read from it.
///
/// These are global options in mpv, read when a file opens, so they have to
/// be put back for the next stream that does not set them — a playlist
/// channel's referrer must not follow the viewer to the next channel, nor
/// the service's token to a provider. An Xtream channel never sets either,
/// and while nothing has set them there is nothing to undo, so on an Xtream
/// account this makes no mpv calls at all.
///
/// Failures are logged rather than returned: a stream that wanted a header
/// may still play without one, and refusing to try would be worse.
fn apply_http_headers(p: &Player, target: &Playable, via: &Via) {
    let token = match via {
        Via::Service { header, .. } => header.as_deref(),
        Via::Direct => None,
    };
    if token.is_some() || SERVICE_HEADER.load(Ordering::Acquire) {
        // Cleared, then the one header appended whole: a list option set
        // from a string would split it at any comma.
        let mut r = p.command("change-list", &["http-header-fields", "clr", ""]);
        if let Some(h) = token {
            r = r.and_then(|()| p.command("change-list", &["http-header-fields", "append", h]));
        }
        if let Err(e) = r {
            tracing::warn!("could not set the recording service's header: {e}");
        }
        SERVICE_HEADER.store(token.is_some(), Ordering::Release);
    }
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

/// "When a channel opens", as last saved or applied: [`LiveStart`] as its
/// index, so it can live in an atomic. The default until the saved
/// settings say otherwise.
static LIVE_START: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(LiveStart::CatchUp as u8);

fn live_start_mode() -> LiveStart {
    match LIVE_START.load(Ordering::Acquire) {
        x if x == LiveStart::Stay as u8 => LiveStart::Stay,
        x if x == LiveStart::Wait as u8 => LiveStart::Wait,
        _ => LiveStart::CatchUp,
    }
}

/// Shared by live and catchup: state first, then the one connection.
async fn open(app: &AppHandle, stream_id: i64, target: Playable, kind: OpenKind) -> Result<(), String> {
    open_as(app, Opened::Channel(stream_id), target, kind, None, Via::Direct, None, None).await
}

/// [`open`], as automatic reconnect number `reconnect` when it is one,
/// from where `via` says, with the audio and subtitle tracks `tracks`
/// (mpv's `aid` and `sid`) rather than mpv's own choice when given: the same
/// channel, moved onto another connection.
///
/// `start` is where in a movie to begin, in seconds; mpv is told on every
/// open, so a channel opened after a resumed movie starts at its beginning.
#[allow(clippy::too_many_arguments)]
async fn open_as(
    app: &AppHandle,
    what: Opened,
    target: Playable,
    kind: OpenKind,
    reconnect: Option<u32>,
    via: Via,
    tracks: Option<(String, String)>,
    start: Option<f64>,
) -> Result<(), String> {
    let left = LAST_OPEN.lock().ok().and_then(|mut last| {
        let before = last.replace((what, target.clone(), kind, via.clone()));
        match before {
            Some((_, _, _, Via::Service { viewer, .. })) if !matches!(&via, Via::Service { viewer: v, .. } if *v == viewer) => {
                Some(viewer)
            }
            _ => None,
        }
    });
    // A place on the service's connection given up at once, before the
    // provider is asked for anything: when nobody else is on it (the
    // recording ended), the service closes it now rather than when its
    // reader has been gone a while, and an account of one connection has it
    // back for this one.
    if let Some(viewer) = left {
        if let Ok(c) = crate::recording::client::Client::local() {
            if let Err(e) = c.stop_watching(&viewer).await {
                tracing::info!("could not give the recording service's place back: {e:#}");
            }
        }
    }
    THROUGH_SERVICE.store(matches!(via, Via::Service { .. }), Ordering::Release);
    ensure_ticker(app);
    // The scrollback previews describe a buffer that is about to be closed.
    // Keeping them would put the previous channel's picture under the cursor.
    crate::surface::trickplay::clear();
    // Same reasoning, for the same reason: the graphs describe one stream.
    crate::ipc::health::clear();
    let mode = live_start_mode();
    let mut was_holding = false;
    let mut join = false;
    commit(app, |m| {
        was_holding = m.joining;
        m.opening(what);
        m.live = kind == OpenKind::Live;
        m.catch_up = match kind {
            OpenKind::CatchUp(c) => Some(c),
            OpenKind::Live | OpenKind::OnDemand => None,
        };
        m.starting(mode);
        join = m.joining;
        match reconnect {
            // Carries on the run: its count, and what started it.
            Some(attempt) => {
                m.reconnects = attempt;
                m.dto.reconnect = Some(ReconnectDto {
                    attempt,
                    of: connections::BACKOFF.len() as u32,
                    checking: false,
                });
            }
            // A person asked: a fresh start.
            None => m.chain_ending = None,
        }
    });
    let r = on_player(app, move |p| {
        apply_http_headers(p, &target, &via);
        apply_start(p, start);
        match &tracks {
            Some((aid, sid)) => {
                for (key, value) in [("aid", aid), ("sid", sid)] {
                    if let Err(e) = p.set_property(key, value) {
                        tracing::warn!("could not keep {key}: {e}");
                    }
                }
            }
            None => reset_track_choice(p),
        }
        // `pause` is global in mpv and outlives the file, so it is set
        // before the open: paused, the stream opens, decodes its first frame
        // and shows it, and the demuxer keeps reading behind it. And put
        // back for an open that does not hold, if the hold on the last
        // channel was cut short by this one: that pause was ours.
        if join || was_holding {
            if let Err(e) = p.set_flag("pause", join) {
                tracing::warn!("could not set up the start at live: {e}");
            }
        }
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
    // Where a movie or an episode got to, before the title is forgotten.
    let place = PROGRESS.lock().ok().and_then(|mut p| p.flush());
    if let Some(w) = place {
        persist_progress(w);
    }
    let r = on_player(&app, |p| p.command("stop", &[]).map_err(|e| e.to_string())).await;
    commit(&app, |m| m.stopped());
    // Its place on the recording service's connection, given back now.
    if let Some(viewer) = current_viewer() {
        if let Ok(mut last) = LAST_OPEN.lock() {
            if let Some(l) = last.as_mut() {
                l.3 = Via::Direct;
            }
        }
        THROUGH_SERVICE.store(false, Ordering::Release);
        if let Ok(c) = crate::recording::client::Client::local() {
            let _ = c.stop_watching(&viewer).await;
        }
    }
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
    let movie = {
        let mut m = STATE.lock().expect("player state mutex poisoned");
        if seconds < 0.0 {
            m.following_live = false;
        }
        m.dto.on_demand.is_some()
    };
    on_player(&app, move |p| {
        let probe = p.probe();
        // A movie is read from a file that can be asked for any part of it:
        // the buffered window is a live stream's limit, not its.
        let by = if movie {
            seconds
        } else {
            clamp_seek(seconds, probe.buffered_back, probe.behind_live)
        };
        if by.abs() < 0.05 {
            return Ok(());
        }
        p.command("seek", &[&format!("{by:.3}"), "relative"])
            .map_err(|e| e.to_string())
    })
    .await
}

/// Where in a movie to seek to, for a place `seconds` from its start:
/// inside the file, which ends where `duration` says when it says.
fn clamp_to_title(seconds: f64, duration: Option<f64>) -> f64 {
    let at = seconds.max(0.0);
    duration.map_or(at, |d| at.min(d))
}

/// Seeks a movie or an episode to `seconds` from its start.
#[tauri::command]
pub async fn seek_to(app: AppHandle, seconds: f64) -> Result<(), String> {
    if !seconds.is_finite() {
        return Err("that is not a time".into());
    }
    let movie = STATE
        .lock()
        .expect("player state mutex poisoned")
        .dto
        .on_demand
        .is_some();
    if !movie {
        return Err("seeking to a time is for movies and episodes".into());
    }
    on_player(&app, move |p| {
        let at = clamp_to_title(seconds, p.get_f64("duration").filter(|d| d.is_finite() && *d > 0.0));
        p.command("seek", &[&format!("{at:.3}"), "absolute"])
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

/// The shortest jump to live the picture announces when the viewer asked
/// for it. A press of Live a moment after the last one moves playback a
/// fraction of a second, which is not something to tell anybody about.
const MIN_REPORTED_JUMP: f64 = 1.0;

#[tauri::command]
pub async fn seek_live(app: AppHandle) -> Result<(), String> {
    // Asking for live means staying there, not visiting once.
    // A movie or an episode has no live edge, and says so.
    let to = {
        let mut m = STATE.lock().expect("player state mutex poisoned");
        let to = m.to_live();
        match to {
            ToLive::Seek => m.follow_live()?,
            ToLive::Reopen(id) => m.reopen_started(id),
            ToLive::Already => {}
        }
        to
    };
    if to == ToLive::Already {
        return Ok(());
    }
    if let ToLive::Reopen(stream_id) = to {
        // A catch-up programme's end is not live: forward through it is
        // only further into the programme. The channel is opened again, as
        // picking it would, once the programme's connection is closed — an
        // account allowed one stream has it back for the channel.
        //
        // The open's own state begins when the open does (`open_as`), not
        // here: a channel that is only being asked for — the recording
        // service may take a while to answer — is not yet an open on the
        // clock, and one that never starts must not end up counted against
        // the channel. Should it not start, the programme is gone either
        // way, and that is said.
        tracing::info!("back to live from a catch-up programme");
        let r = match on_player(&app, |p| p.command("stop", &[]).map_err(|e| e.to_string())).await {
            Ok(()) => play_channel(&app, stream_id).await,
            Err(e) => Err(e),
        };
        if let Err(e) = &r {
            commit(&app, |m| m.reopen_failed(stream_id, e));
        }
        return r;
    }
    let by = on_player(&app, |p| {
        let probe = p.probe();
        // Relative, the same operation the backward seek already does, in
        // reverse - and bounded by `behind_live`, which is measured to the end
        // of mpv's *seekable* window rather than to the end of its cache. A
        // seek past the seekable end (the cache runs one to three seconds
        // beyond it) is ignored by mpv without an error: playback stays
        // exactly where it was.
        let by = (probe.behind_live - LIVE_CUSHION).max(0.0);
        if by <= 0.05 {
            return Ok(0.0);
        }
        p.command("seek", &[&format!("{by:.3}"), "relative"])
            .map(|()| by)
            .map_err(|e| e.to_string())
    })
    .await?;
    commit(&app, |m| m.jumped_to_live(by));
    Ok(())
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

/// Where mpv starts the file it is about to open. Set by every open, since
/// the option outlives the file: a channel opened after a resumed movie must
/// not begin where the movie did.
fn apply_start(p: &Player, start: Option<f64>) {
    let value = match start.filter(|s| s.is_finite() && *s > 0.0) {
        Some(secs) => format!("{secs:.1}"),
        None => "none".to_owned(),
    };
    if let Err(e) = p.set_property("start", &value) {
        tracing::warn!("could not set where to start: {e}");
    }
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

/// Which decoder the log says is at work, from mpv's `hwdec-current`: absent,
/// empty or `"no"` is software.
fn decoder_words(hwdec_current: Option<&str>) -> String {
    match hwdec_current.map(str::trim) {
        None | Some("" | "no") => "software (no hardware decoder)".to_owned(),
        Some(h) => format!("hardware ({h})"),
    }
}

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
    let (y, m, d) = crate::localtime::civil_from_days(t.div_euclid(86_400));
    let s = t.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}:{:02}-{:02}",
        s / 3600,
        (s % 3600) / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::{SettingsDto, SubtitleBackdrop, SubtitleStyleDto};

    #[test]
    fn quitting_writes_the_place_a_movie_got_to_and_only_once() {
        let c = crate::catalog::open_in_memory().unwrap();
        let mut saver = ProgressSaver::new();
        // Two seconds into a movie: well short of a periodic save, so only a
        // flush at quit would keep this place.
        let movie = Some(VodItem::movie(7));
        assert!(saver.observe(movie, Some(1234.5), Some(5400.0), true, 100.0).is_none());
        assert!(saver.observe(movie, Some(1236.5), Some(5400.0), true, 102.0).is_none());
        let marks = flush_progress(&mut saver, &c).unwrap().expect("a write at quit");
        assert_eq!((marks.position, marks.duration), (Some(1236.5), Some(5400.0)));
        let stored: f64 = c
            .query_row("SELECT position FROM vod_marks WHERE item_id = 7", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, 1236.5);
        // Nothing new since: nothing more to write.
        assert!(flush_progress(&mut saver, &c).unwrap().is_none());
    }

    #[test]
    fn quitting_writes_the_place_an_episode_got_to_on_the_episode() {
        let c = crate::catalog::open_in_memory().unwrap();
        let mut saver = ProgressSaver::new();
        let episode = Some(VodItem::episode(7));
        assert!(saver.observe(episode, Some(600.0), Some(1500.0), true, 100.0).is_none());
        assert!(saver.observe(episode, Some(602.0), Some(1500.0), true, 102.0).is_none());
        let marks = flush_progress(&mut saver, &c).unwrap().expect("a write at quit");
        assert_eq!((marks.position, marks.duration), (Some(602.0), Some(1500.0)));
        let kinds: Vec<String> = c
            .prepare("SELECT kind FROM vod_marks WHERE item_id = 7")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(kinds, ["episode"]);
    }

    #[test]
    fn the_decoder_log_reads_hwdec_no_as_software() {
        let software = "software (no hardware decoder)";
        assert_eq!(decoder_words(None), software);
        assert_eq!(decoder_words(Some("no")), software);
        assert_eq!(decoder_words(Some("")), software);
        assert_eq!(decoder_words(Some("vaapi")), "hardware (vaapi)");
    }

    #[test]
    fn state_walks_stopped_to_opening_to_playing() {
        let mut m = Machine::new();
        assert_eq!(m.dto.state, PlaybackState::Stopped);

        m.opening(Opened::Channel(498_338));
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
        m.opening(Opened::Channel(1));
        m.failed("connection refused");
        assert_eq!(m.dto.state, PlaybackState::Failed);
        assert_eq!(m.dto.error.as_deref(), Some("connection refused"));
    }

    #[test]
    fn an_open_that_never_decodes_fails_on_the_deadline() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
        m.opened_at = Some(Instant::now() - FIRST_FRAME_DEADLINE - Duration::from_secs(1));
        m.check_deadline();
        assert_eq!(m.dto.state, PlaybackState::Failed);
        assert_eq!(m.dto.error.as_deref(), Some("no video within 8s"));
    }

    #[test]
    fn the_deadline_does_not_fire_on_a_playing_stream() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
        m.playing();
        m.opened_at = Some(Instant::now() - Duration::from_secs(600));
        m.check_deadline();
        assert_eq!(m.dto.state, PlaybackState::Playing);
    }

    fn a_playing_movie(id: i64) -> Machine {
        let mut m = Machine::new();
        m.opening(Opened::Movie(id));
        m.live = false;
        m.starting(LiveStart::CatchUp);
        m.playing();
        m
    }

    #[test]
    fn a_movie_open_reports_no_channel() {
        let mut m = Machine::new();
        m.opening(Opened::Movie(7));
        assert_eq!(m.dto.stream_id, None);
        assert_eq!(
            m.dto.on_demand.as_ref().map(|o| (o.kind, o.id)),
            Some((OnDemandKind::Movie, 7))
        );
        m.opening(Opened::Channel(7));
        assert_eq!((m.dto.stream_id, m.dto.on_demand.is_none()), (Some(7), true));
    }

    #[test]
    fn a_movie_reports_its_place_in_whole_seconds() {
        let mut m = a_playing_movie(1);
        m.apply(&Probe {
            time_pos: Some(12.6),
            duration: Some(5400.4),
            cache_end: Some(80.2),
            ..Probe::default()
        });
        let od = m.dto.on_demand.unwrap();
        assert_eq!(od.position, 13.0);
        assert_eq!(od.duration, Some(5400.0));
        assert_eq!(od.buffered_to, Some(80.0));
        assert!(!m.dto.at_live_edge);
        assert!(!m.following_live, "nothing chases an edge in a film");
    }

    #[test]
    fn a_movie_earns_nothing_for_a_channel() {
        let mut m = a_playing_movie(7);
        assert!(m.ledger(Instant::now() + Duration::from_secs(10)).is_none());
        let mut watched = Accumulator::new();
        let mut earned = None;
        for t in 0..120 {
            earned = earned.or(watched.observe(m.dto.stream_id, true, 1.0, t));
        }
        assert!(earned.is_none());
    }

    #[test]
    fn a_movie_that_plays_to_its_end_stops_rather_than_failing() {
        let mut m = a_playing_movie(7);
        m.shown = true;
        m.apply(&Probe {
            time_pos: Some(119.6),
            duration: Some(120.0),
            ..Probe::default()
        });
        m.ended_with(How::Eof);
        assert_eq!(m.dto.state, PlaybackState::Stopped);
        assert!(m.dto.ending.is_none());
    }

    #[test]
    fn a_movie_is_never_reconnected() {
        let network = Ending {
            kind: EndingKind::Network,
            status: None,
            limit: false,
            slots: None,
            clip_secs: None,
        };
        let mut live = Machine::new();
        live.opening(Opened::Channel(7));
        live.plan_reconnect(network, false);
        assert!(live.reconnect_due.is_some(), "a channel would be");
        let mut m = a_playing_movie(7);
        m.plan_reconnect(network, false);
        assert!(m.reconnect_due.is_none());
    }

    #[test]
    fn an_end_of_file_in_the_middle_of_a_movie_is_a_failure_that_keeps_its_place() {
        let mut m = a_playing_movie(7);
        m.shown = true;
        m.apply(&Probe {
            time_pos: Some(1200.0),
            duration: Some(5400.0),
            ..Probe::default()
        });
        m.ended_with(How::Eof);
        assert_eq!(m.dto.state, PlaybackState::Failed);
        assert!(m.dto.ending.is_some());
        assert!(m.reconnect_due.is_none());
        let od = m.dto.on_demand.unwrap();
        assert_eq!((od.id, od.position), (7, 1200.0));
    }

    #[test]
    fn an_end_of_file_with_no_known_length_is_a_failure() {
        let mut m = a_playing_movie(7);
        m.shown = true;
        if let Some(od) = m.dto.on_demand.as_mut() {
            od.position = 0.0;
            od.duration = Some(0.0);
        }
        m.ended_with(How::Eof);
        assert_eq!(m.dto.state, PlaybackState::Failed);
    }

    #[test]
    fn an_end_of_file_at_the_end_of_a_movie_stops_it() {
        let mut m = a_playing_movie(7);
        m.shown = true;
        m.apply(&Probe {
            time_pos: Some(5395.0),
            duration: Some(5400.0),
            ..Probe::default()
        });
        m.ended_with(How::Eof);
        assert_eq!(m.dto.state, PlaybackState::Stopped);
    }

    #[test]
    fn a_movie_has_no_live_edge_to_seek_to() {
        let mut m = a_playing_movie(7);
        assert!(m.follow_live().is_err());
        assert!(!m.following_live, "catch-up must not be armed for a film");
        m.opening(Opened::Channel(1));
        m.following_live = false;
        assert!(m.follow_live().is_ok() && m.following_live);
    }

    #[test]
    fn a_movie_is_never_taken_for_a_catch_up_programme() {
        // Not live, as a programme from an archive is not, but not from one
        // either: Live has no channel to reopen, and a refusal is the
        // movie's, never the archive's.
        let mut m = a_playing_movie(7);
        assert_eq!(m.catch_up, None);
        assert_eq!(m.to_live(), ToLive::Seek);
        assert!(m.follow_live().is_err());
        m.clues.observe("http: HTTP error 404 Not Found");
        m.ended_with(How::Error(-13));
        assert_eq!(m.dto.ending.map(|e| e.kind), Some(EndingKind::Unavailable));
        assert_eq!(m.to_live(), ToLive::Seek);
        // One slow to its first frame plays when it comes, with nothing of
        // a live open about it.
        let mut m = Machine::new();
        m.opening(Opened::Movie(7));
        m.live = false;
        m.starting(LiveStart::Wait);
        m.ended_with(How::Deadline);
        assert!(m.reconfigured(true));
        assert_eq!(m.dto.state, PlaybackState::Playing);
        assert!(m.join.is_none() && m.settle.is_none() && !m.following_live);
    }

    fn a_playing_episode(id: i64) -> Machine {
        let mut m = Machine::new();
        m.opening(Opened::Episode(id));
        m.live = false;
        m.starting(LiveStart::CatchUp);
        m.playing();
        m
    }

    #[test]
    fn an_episode_open_reports_no_channel_and_no_movie() {
        let mut m = Machine::new();
        m.opening(Opened::Episode(7));
        assert_eq!(m.dto.stream_id, None);
        assert_eq!(
            m.dto.on_demand.as_ref().map(|o| (o.kind, o.id)),
            Some((OnDemandKind::Episode, 7))
        );
        assert!(!m.dto.at_live_edge && !m.following_live);
        m.opening(Opened::Movie(7));
        assert_eq!(
            m.dto.on_demand.as_ref().map(|o| (o.kind, o.id)),
            Some((OnDemandKind::Movie, 7))
        );
    }

    #[test]
    fn an_episode_earns_nothing_for_a_channel() {
        let mut m = a_playing_episode(7);
        assert!(m.ledger(Instant::now() + Duration::from_secs(10)).is_none());
        let mut watched = Accumulator::new();
        let mut earned = None;
        for t in 0..120 {
            earned = earned.or(watched.observe(m.dto.stream_id, true, 1.0, t));
        }
        assert!(earned.is_none());
    }

    #[test]
    fn an_episode_is_never_reconnected() {
        let network = Ending {
            kind: EndingKind::Network,
            status: None,
            limit: false,
            slots: None,
            clip_secs: None,
        };
        let mut m = a_playing_episode(7);
        m.plan_reconnect(network, false);
        assert!(m.reconnect_due.is_none());
        assert!(m.follow_live().is_err());
        assert_eq!(m.to_live(), ToLive::Seek);
    }

    #[test]
    fn an_episode_that_plays_to_its_end_stops_rather_than_failing() {
        let mut m = a_playing_episode(7);
        m.shown = true;
        m.apply(&Probe {
            time_pos: Some(119.6),
            duration: Some(120.0),
            ..Probe::default()
        });
        m.ended_with(How::Eof);
        assert_eq!(m.dto.state, PlaybackState::Stopped);
        assert!(m.dto.ending.is_none());
        // One cut off in its middle keeps its place, as a movie does.
        let mut m = a_playing_episode(7);
        m.shown = true;
        m.apply(&Probe {
            time_pos: Some(60.0),
            duration: Some(120.0),
            ..Probe::default()
        });
        m.ended_with(How::Eof);
        assert_eq!(m.dto.state, PlaybackState::Failed);
        let od = m.dto.on_demand.unwrap();
        assert_eq!((od.kind, od.id, od.position), (OnDemandKind::Episode, 7, 60.0));
    }

    #[test]
    fn an_episode_is_written_down_as_an_episode() {
        let od = OnDemandDto {
            kind: OnDemandKind::Episode,
            id: 7,
            position: 0.0,
            duration: None,
            buffered_to: None,
        };
        assert_eq!(item_of(&od), VodItem::episode(7));
        assert_eq!(item_of(&OnDemandDto { kind: OnDemandKind::Movie, ..od }), VodItem::movie(7));
    }

    #[test]
    fn a_seek_to_stays_inside_the_title() {
        assert_eq!(clamp_to_title(-5.0, Some(100.0)), 0.0);
        assert_eq!(clamp_to_title(500.0, Some(100.0)), 100.0);
        assert_eq!(clamp_to_title(42.0, Some(100.0)), 42.0);
        assert_eq!(clamp_to_title(500.0, None), 500.0);
    }

    #[test]
    fn a_zap_clears_the_previous_streams_measurements() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
        m.playing();
        m.apply(&Probe {
            width: Some(1920),
            height: Some(1080),
            codec: Some("h264".into()),
            ..Probe::default()
        });
        assert_eq!(m.dto.width, Some(1920));
        m.opening(Opened::Channel(2));
        assert_eq!(
            m.dto.width, None,
            "channel names lie; stale numbers lie too"
        );
        assert_eq!(m.dto.codec, None);
    }

    #[test]
    fn an_automatic_catch_up_is_reported_with_its_distance() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
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
    fn a_jump_the_viewer_asked_for_is_reported_too() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
        m.playing();
        m.drift_secs = 4.0;
        m.jumped_to_live(11.96);
        assert_eq!(m.dto.live_jump, Some(LiveJumpDto { seconds: 12.0, seq: 1 }));
        assert_eq!(m.drift_secs, 0.0, "at live, nothing is lost");
        assert!(
            m.last_live_nudge.is_none(),
            "asking for live must not hold off the catch-up that keeps it there"
        );
        assert!(m.changed().is_some_and(|d| d.live_jump.is_some()));
    }

    #[test]
    fn a_tiny_jump_the_viewer_asked_for_is_not_news() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
        m.playing();
        m.jumped_to_live(0.6);
        assert_eq!(m.dto.live_jump, None, "under a second is not worth a notice");
        m.jumped_to_live(MIN_REPORTED_JUMP);
        assert_eq!(m.dto.live_jump.map(|j| j.seq), Some(1), "a second is");
        m.jumped_to_live(0.2);
        assert_eq!(
            m.dto.live_jump.map(|j| j.seq),
            Some(1),
            "and a tiny one after it neither repeats nor clears it"
        );
    }

    #[test]
    fn a_second_jump_of_the_same_size_is_still_news() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
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
        m.opening(Opened::Channel(1));
        m.playing();
        m.caught_up(9.0);
        m.opening(Opened::Channel(2));
        assert_eq!(m.dto.live_jump, None, "a zap starts with no news");
        m.playing();
        m.caught_up(4.0);
        m.stopped();
        assert_eq!(m.dto.live_jump, None);
        // Counted across the session, so a jump on the next stream cannot
        // repeat a number the frontend has already announced.
        m.opening(Opened::Channel(3));
        m.playing();
        m.caught_up(4.0);
        assert_eq!(m.dto.live_jump.map(|j| j.seq), Some(3));
    }

    /// A live channel opened to start at live, as `open` sets it up.
    fn held(id: i64) -> Machine {
        let mut m = Machine::new();
        m.opening(Opened::Channel(id));
        m.starting(LiveStart::Wait);
        m
    }

    #[test]
    fn catching_up_plays_at_once_and_jumps_after_the_burst() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
        m.starting(LiveStart::CatchUp);
        assert!(!m.joining, "no hold");
        assert!(m.following_live, "the catch-up after the burst runs");
        m.playing();
        assert_eq!(m.dto.state, PlaybackState::Playing, "moving from the first frame");
        assert!(m.settle.is_some());
    }

    #[test]
    fn staying_put_never_skips_ahead_by_itself() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
        m.starting(LiveStart::Stay);
        assert!(!m.joining);
        assert!(!m.following_live, "neither after the burst nor after a stall");
        m.playing();
        assert_eq!(m.dto.state, PlaybackState::Playing);
    }

    #[test]
    fn waiting_holds_the_first_frame() {
        let mut m = held(1);
        assert!(m.joining && m.following_live);
        m.playing();
        assert_eq!(m.dto.state, PlaybackState::Opening);
        assert!(m.dto.joining_live);
    }

    #[test]
    fn a_catch_up_programme_starts_where_it_was_asked_whatever_the_setting() {
        for mode in [LiveStart::Stay, LiveStart::CatchUp, LiveStart::Wait] {
            let mut m = Machine::new();
            m.opening(Opened::Channel(1));
            m.live = false;
            m.starting(mode);
            assert!(!m.joining, "{mode:?}");
            m.playing();
            assert_eq!(m.dto.state, PlaybackState::Playing, "{mode:?}");
        }
    }

    /// A catch-up programme opened as `play_catchup` opens it.
    fn catchup(id: i64, mode: LiveStart) -> Machine {
        catchup_at(id, mode, true)
    }

    /// The same, asked for near the archive's far end or not.
    fn catchup_at(id: i64, mode: LiveStart, near_edge: bool) -> Machine {
        let mut m = Machine::new();
        m.opening(Opened::Channel(id));
        m.live = false;
        m.catch_up = Some(CatchUp { near_edge });
        m.starting(mode);
        m
    }

    #[test]
    fn a_bare_refusal_well_inside_the_archive_is_still_the_limits() {
        let mut m = catchup_at(42, LiveStart::CatchUp, false);
        m.clues.observe("http: HTTP error 403 Forbidden");
        m.ended_with(How::Error(-13));
        assert_eq!(m.dto.ending.map(|e| e.kind), Some(EndingKind::Refused));
        let mut m = catchup_at(42, LiveStart::CatchUp, true);
        m.clues.observe("http: HTTP error 403 Forbidden");
        m.ended_with(How::Error(-13));
        assert_eq!(m.dto.ending.map(|e| e.kind), Some(EndingKind::BeyondArchive));
    }

    #[test]
    fn near_the_edge_is_the_archives_first_hour_or_any_of_one_of_unknown_depth() {
        let earliest = 1_000_000;
        assert!(near_edge(earliest, Some(earliest)));
        assert!(near_edge(earliest + NEAR_EDGE - 60, Some(earliest)));
        assert!(!near_edge(earliest + NEAR_EDGE, Some(earliest)));
        assert!(!near_edge(earliest + 86_400, Some(earliest)));
        assert!(near_edge(earliest + 86_400, None));
    }

    #[test]
    fn only_a_catch_up_programme_is_explained_by_the_archive_or_reopened_live() {
        // Not live, but not from an archive either (a film, say).
        let mut m = Machine::new();
        m.opening(Opened::Channel(42));
        m.live = false;
        m.starting(LiveStart::CatchUp);
        assert_eq!(m.to_live(), ToLive::Seek);
        m.clues.observe("http: HTTP error 404 Not Found");
        m.ended_with(How::Error(-13));
        assert_eq!(m.dto.ending.map(|e| e.kind), Some(EndingKind::Unavailable));
        // And a live open clears what the last one was.
        let mut m = catchup(42, LiveStart::CatchUp);
        m.opening(Opened::Channel(42));
        assert_eq!(m.catch_up, None);
    }

    #[test]
    fn a_live_reopen_that_never_starts_leaves_nothing_against_the_channel() {
        // `seek_live` from a catch-up commits nothing before the channel's
        // own open; if that never comes, the programme is reported failed.
        let mut m = catchup(42, LiveStart::CatchUp);
        m.playing();
        m.reopen_started(42);
        m.reopen_failed(42, "the recording service did not answer");
        assert_eq!(m.dto.state, PlaybackState::Failed);
        assert!(!m.live, "still the programme's state, not a live open's");
        m.failed_since = Some(Instant::now() - FAILURE_SETTLE - Duration::from_secs(1));
        m.opened_at = Some(Instant::now() - FIRST_FRAME_DEADLINE - Duration::from_secs(1));
        m.check_deadline();
        assert_eq!(m.ledger(Instant::now()), None, "a catch-up says nothing of the channel");
        assert_eq!(m.to_live(), ToLive::Reopen(42), "and Live can try again");
    }

    #[test]
    fn a_second_live_press_during_the_reopen_is_nothing() {
        let mut m = catchup(42, LiveStart::CatchUp);
        m.playing();
        assert_eq!(m.to_live(), ToLive::Reopen(42));
        m.reopen_started(42);
        assert_eq!(m.to_live(), ToLive::Already);
        // The channel's open begins: Live is a seek again.
        m.opening(Opened::Channel(42));
        m.starting(LiveStart::CatchUp);
        assert_eq!(m.to_live(), ToLive::Seek);
    }

    #[test]
    fn a_failed_reopen_does_not_land_on_what_came_after_it() {
        // Another channel picked while the reopen was being asked for.
        let mut m = catchup(42, LiveStart::CatchUp);
        m.playing();
        m.reopen_started(42);
        m.opening(Opened::Channel(7));
        m.starting(LiveStart::CatchUp);
        m.reopen_failed(42, "too late");
        assert_eq!(m.dto.state, PlaybackState::Opening);
        assert_eq!(m.dto.stream_id, Some(7));
        // Or the player stopped.
        let mut m = catchup(42, LiveStart::CatchUp);
        m.playing();
        m.reopen_started(42);
        m.stopped();
        m.reopen_failed(42, "too late");
        assert_eq!(m.dto.state, PlaybackState::Stopped);
    }

    #[test]
    fn a_revived_open_starts_as_its_first_frame_would_have() {
        // Under "wait": held at live once it comes, and noted as loaded.
        let mut m = held(7);
        m.ended_with(How::Deadline);
        m.failed_since = Some(Instant::now() - FAILURE_SETTLE - Duration::from_secs(1));
        assert!(matches!(m.ledger(Instant::now()), Some(Ledger::Failed(7, _))));
        assert!(m.reconfigured(true));
        assert!(m.join.is_some() && m.dto.joining_live, "held, to be let go at live");
        assert_eq!(m.dto.state, PlaybackState::Opening);
        assert!(m.dto.ending.is_none() && m.dto.error.is_none());
        m.joined();
        assert_eq!(m.dto.state, PlaybackState::Playing);
        assert!(m.settle.is_some() && m.measure_at.is_some() && m.following_live);
        assert_eq!(m.ledger(Instant::now()), Some(Ledger::Loaded(7)));
        // Played at once: caught up after its burst, and measured.
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.starting(LiveStart::CatchUp);
        m.ended_with(How::Deadline);
        m.playing();
        assert_eq!(m.dto.state, PlaybackState::Playing);
        assert!(m.settle.is_some() && m.measure_at.is_some() && m.following_live);
        assert_eq!(m.ledger(Instant::now()), Some(Ledger::Loaded(7)));
    }

    #[test]
    fn an_open_given_up_on_without_a_word_is_noted_when_it_plays() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.starting(LiveStart::CatchUp);
        m.failed("the player did not answer");
        m.failed_since = Some(Instant::now() - FAILURE_SETTLE - Duration::from_secs(1));
        assert_eq!(m.ledger(Instant::now()), None, "nothing to judge without an ending");
        m.file_started();
        assert!(m.reconfigured(true));
        assert_eq!(m.ledger(Instant::now()), Some(Ledger::Loaded(7)));
    }

    #[test]
    fn a_station_whose_track_list_mpv_never_resends_is_read_for_it() {
        // Radio to radio from one provider: the same track list, so no
        // change is reported, and the heartbeat has to ask.
        let mut m = Machine::new();
        m.opening(Opened::Channel(2));
        m.starting(LiveStart::CatchUp);
        m.file_started();
        m.tracks_known(None);
        m.audio(true);
        assert!(m.needs_track_list());
        assert_eq!(m.dto.state, PlaybackState::Opening);
        // What the heartbeat reads instead.
        m.tracks_known(tracks::has_video(r#"[{"id":1,"type":"audio","codec":"aac"}]"#));
        assert!(!m.needs_track_list());
        assert_eq!(m.dto.state, PlaybackState::Playing);
        // Not asked before the file has started, nor once known.
        let mut m = Machine::new();
        m.opening(Opened::Channel(2));
        assert!(!m.needs_track_list());
    }

    #[test]
    fn a_slow_archives_refusal_after_the_deadline_still_explains_it() {
        let mut m = catchup(42, LiveStart::CatchUp);
        m.ended_with(How::Deadline);
        assert_eq!(m.dto.ending.map(|e| e.kind), Some(EndingKind::NoVideo));
        m.heard("http: HTTP error 404 Not Found");
        m.ended_with(How::Error(-13));
        let e = m.dto.ending.unwrap();
        assert_eq!((e.kind, e.status), (EndingKind::BeyondArchive, Some(404)));
        // Once explained, later lines are not evidence of anything.
        m.heard("http: HTTP error 509 Max Connections Reached");
        assert_eq!(m.clues.status, Some(404));
    }

    #[test]
    fn a_radio_channel_plays_on_its_sound() {
        for mode in [LiveStart::CatchUp, LiveStart::Stay] {
            let mut m = Machine::new();
            m.opening(Opened::Channel(7));
            m.starting(mode);
            m.file_started();
            m.tracks_known(Some(false));
            assert_eq!(m.dto.state, PlaybackState::Opening, "{mode:?}: not before the sound");
            m.audio(true);
            assert_eq!(m.dto.state, PlaybackState::Playing, "{mode:?}");
            assert!(m.opened_at.is_none(), "{mode:?}: the deadline is disarmed");
            assert_eq!(m.ledger(Instant::now()), Some(Ledger::Loaded(7)), "{mode:?}");
        }
        // Started at live: the hold begins on the first sound.
        let mut m = held(7);
        m.file_started();
        m.audio(true);
        m.tracks_known(Some(false));
        assert!(m.join.is_some() && m.dto.joining_live);
    }

    #[test]
    fn sound_alone_is_not_a_tv_channels_picture() {
        // A channel with video that has sound but no frame yet is opening.
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.file_started();
        m.tracks_known(Some(true));
        m.audio(true);
        assert_eq!(m.dto.state, PlaybackState::Opening);
        // Nor before this open's file started, nor before its tracks are
        // known: the last stream's sound may still be going.
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.tracks_known(Some(false));
        m.audio(true);
        assert_eq!(m.dto.state, PlaybackState::Opening);
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.file_started();
        m.audio(true);
        m.tracks_known(None);
        assert_eq!(m.dto.state, PlaybackState::Opening);
        // A radio channel that only got going after the deadline plays.
        m.ended_with(How::Deadline);
        m.tracks_known(Some(false));
        assert_eq!(m.dto.state, PlaybackState::Playing);
    }

    #[test]
    fn an_open_given_up_on_without_a_word_plays_when_its_picture_comes() {
        // The player did not answer in time, so the open was reported
        // failed, but mpv went on and started the file.
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.failed("the player did not answer");
        m.file_started();
        assert!(m.reconfigured(true));
        assert_eq!(m.dto.state, PlaybackState::Playing);
    }

    #[test]
    fn a_catch_up_programme_is_never_caught_up_to_its_end() {
        for mode in [LiveStart::Stay, LiveStart::CatchUp, LiveStart::Wait] {
            let mut m = catchup(1, mode);
            assert!(!m.following_live, "{mode:?}: nothing follows the end of a programme");
            m.playing();
            assert!(m.settle.is_none(), "{mode:?}: no catch-up after a burst it does not have");
            assert!(!m.following_live, "{mode:?}");
            // Its buffer reads deep, as an archive served faster than
            // realtime does: still nothing to chase.
            m.apply(&Probe {
                behind_live: 29.7,
                ..Probe::default()
            });
            assert!(!m.following_live && m.settle.is_none(), "{mode:?}");
        }
        // A live channel still gets both.
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
        m.starting(LiveStart::CatchUp);
        m.playing();
        assert!(m.following_live && m.settle.is_some());
    }

    #[test]
    fn a_catch_up_programme_is_never_at_the_live_edge() {
        let mut m = catchup(1, LiveStart::CatchUp);
        assert!(!m.dto.at_live_edge, "Live is offered while it opens");
        m.playing();
        m.apply(&Probe {
            behind_live: 1.0,
            ..Probe::default()
        });
        assert!(!m.dto.at_live_edge, "however near the end of what it has read");
        // Nor when it fails, so Live can still leave it.
        m.ended_with(How::Error(-13));
        assert!(!m.dto.at_live_edge);
    }

    #[test]
    fn live_from_a_catch_up_programme_opens_the_channel_again() {
        for mode in [LiveStart::Stay, LiveStart::CatchUp, LiveStart::Wait] {
            let mut m = catchup(42, mode);
            assert_eq!(m.to_live(), ToLive::Reopen(42), "{mode:?}: while it opens");
            m.playing();
            assert_eq!(m.to_live(), ToLive::Reopen(42), "{mode:?}: while it plays");
            m.ended_with(How::Error(-13));
            assert_eq!(m.to_live(), ToLive::Reopen(42), "{mode:?}: after it failed");
        }
        // Live, it is a seek within the stream, as it always was.
        let mut m = Machine::new();
        m.opening(Opened::Channel(42));
        m.starting(LiveStart::CatchUp);
        m.playing();
        assert_eq!(m.to_live(), ToLive::Seek);
        // And with nothing open there is nothing to reopen.
        let mut m = catchup(42, LiveStart::CatchUp);
        m.stopped();
        assert_eq!(m.to_live(), ToLive::Seek);
    }

    #[test]
    fn reopening_live_from_a_catch_up_is_a_live_open() {
        let mut m = catchup(42, LiveStart::CatchUp);
        m.playing();
        // What `seek_live` does before the channel opens again.
        m.opening(Opened::Channel(42));
        m.starting(LiveStart::CatchUp);
        assert!(m.live && m.following_live && m.dto.at_live_edge);
        m.playing();
        assert!(m.settle.is_some(), "the channel's connect burst is caught up as on any open");
        assert_eq!(m.to_live(), ToLive::Seek);
    }

    #[test]
    fn a_catch_up_too_old_for_the_archive_says_so() {
        let mut m = catchup(42, LiveStart::CatchUp);
        m.clues.observe("http: HTTP error 404 Not Found");
        m.ended_with(How::Error(-13));
        assert_eq!(m.dto.ending.map(|e| e.kind), Some(EndingKind::BeyondArchive));
        // The same answer to a live channel is about the channel.
        let mut m = Machine::new();
        m.opening(Opened::Channel(42));
        m.clues.observe("http: HTTP error 404 Not Found");
        m.ended_with(How::Error(-13));
        assert_eq!(m.dto.ending.map(|e| e.kind), Some(EndingKind::Unavailable));
    }

    #[test]
    fn the_setting_survives_its_trip_through_the_atomic() {
        for mode in [LiveStart::Stay, LiveStart::CatchUp, LiveStart::Wait] {
            LIVE_START.store(mode as u8, Ordering::Release);
            assert_eq!(live_start_mode(), mode);
        }
        LIVE_START.store(LiveStart::CatchUp as u8, Ordering::Release);
    }

    #[test]
    fn a_held_first_frame_is_not_yet_playing() {
        let mut m = held(1);
        m.playing();
        assert_eq!(m.dto.state, PlaybackState::Opening, "nothing moves yet");
        assert!(m.dto.joining_live, "and the interface is told why");
        assert!(m.shown, "but a frame was shown: an ending now is a drop");
        assert!(m.opened_at.is_none(), "and the first-frame deadline is met");
        m.check_deadline();
        assert_eq!(m.dto.state, PlaybackState::Opening);
    }

    #[test]
    fn letting_go_of_the_hold_plays_and_keeps_watching_for_a_long_burst() {
        let mut m = held(1);
        m.playing();
        m.joined();
        assert_eq!(m.dto.state, PlaybackState::Playing);
        assert!(!m.dto.joining_live);
        assert!(m.join.is_none());
        assert!(m.settle.is_some(), "a burst that outlasted the hold is still caught up");
        // A later reconfigure is a resolution change, not a new hold.
        m.playing();
        assert_eq!(m.dto.state, PlaybackState::Playing);
        assert!(m.join.is_none());
    }

    #[test]
    fn a_reconfigure_during_the_hold_does_not_restart_it() {
        let mut m = held(1);
        m.playing();
        let _ = m.join.as_mut().unwrap().observe(0.25, Some(20.0), 20.0);
        m.playing();
        let j = m.join.as_ref().unwrap();
        assert!(j.saw_burst(), "the same hold, with what it has seen");
    }

    #[test]
    fn the_hold_does_not_report_a_position() {
        // The held frame is about to be skipped past; "24s behind" for the
        // moment before it is would announce the jump this exists to hide.
        let mut m = held(1);
        m.playing();
        m.apply(&Probe {
            width: Some(1280),
            behind_live: 24.0,
            paused: true,
            ..Probe::default()
        });
        assert_eq!(m.dto.width, Some(1280));
        assert_eq!(m.dto.behind_live, 0.0);
        assert!(m.dto.at_live_edge);
        assert_eq!(m.dto.state, PlaybackState::Opening, "not Paused: nobody paused it");
    }

    #[test]
    fn an_ending_during_the_hold_is_explained_like_any_other() {
        let mut m = held(1);
        m.playing();
        m.ended();
        assert_eq!(m.dto.state, PlaybackState::Failed);
        assert!(!m.dto.joining_live);
        assert!(!m.joining);
        assert_eq!(m.dto.ending.unwrap().kind, ending::EndingKind::Dropped);
    }

    #[test]
    fn a_zap_forgets_the_last_hold() {
        let mut m = held(1);
        m.playing();
        m.opening(Opened::Channel(2));
        assert!(!m.joining && m.join.is_none() && !m.dto.joining_live);
    }

    /// A live channel that played and then lost the network.
    fn network_failure(m: &mut Machine) {
        m.clues.observe("tcp: Connection to tcp://10.0.0.1:80 failed: Connection timed out");
        m.ended_with(How::Error(-13));
    }

    #[test]
    fn the_network_failing_is_reconnected_after_a_wait() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.playing();
        network_failure(&mut m);
        assert_eq!(m.dto.state, PlaybackState::Failed);
        assert_eq!(m.dto.ending.unwrap().kind, EndingKind::Network);
        let r = m.dto.reconnect.expect("reconnecting");
        assert_eq!((r.attempt, r.of, r.checking), (1, 4, false));
        let due = m.reconnect_due.expect("a reconnect is due");
        assert!((1.5..=2.5).contains(&due.delay.as_secs_f64()), "{:?}", due.delay);
        assert!(!due.unreached, "it had connected: a frame was shown");
    }

    #[test]
    fn nothing_the_provider_did_is_reconnected() {
        for (line, how) in [
            (None, How::Eof),
            (Some("HTTP error 509 Max Connections Reached"), How::Error(-13)),
            (Some("HTTP error 403 Forbidden"), How::Error(-13)),
            (Some("HTTP error 503 Service Unavailable"), How::Error(-13)),
        ] {
            let mut m = Machine::new();
            m.opening(Opened::Channel(7));
            m.playing();
            if let Some(l) = line {
                m.clues.observe(l);
            }
            m.ended_with(how);
            assert!(m.dto.reconnect.is_none() && m.reconnect_due.is_none(), "{line:?}");
        }
    }

    #[test]
    fn a_catch_up_programme_is_not_reconnected() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.live = false;
        m.playing();
        network_failure(&mut m);
        assert!(m.reconnect_due.is_none());
    }

    #[test]
    fn a_failure_that_never_reached_the_provider_says_so() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.clues.observe("tcp: Failed to resolve hostname nowhere.invalid: Name or service not known");
        m.ended_with(How::Error(-13));
        assert!(m.reconnect_due.unwrap().unreached);
    }

    #[test]
    fn a_reconnect_that_is_refused_stops_there() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.playing();
        network_failure(&mut m);
        // The reconnect, as `open_as` makes it.
        m.opening(Opened::Channel(7));
        m.reconnects = 1;
        m.clues.observe("HTTP error 509 Max Connections Reached");
        m.ended_with(How::Error(-13));
        assert_eq!(m.dto.ending.unwrap().kind, EndingKind::Refused);
        assert!(m.dto.reconnect.is_none(), "a refusal ends the run");
        assert!(m.reconnect_due.is_none());
    }

    #[test]
    fn the_allowance_runs_out() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.reconnects = 4;
        network_failure(&mut m);
        assert!(m.reconnect_due.is_none(), "four is all");
        assert!(m.dto.reconnect.is_none());
        assert_eq!(m.dto.ending.unwrap().kind, EndingKind::Network);
    }

    #[test]
    fn an_account_that_cannot_be_asked_costs_an_attempt() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.playing();
        network_failure(&mut m);
        m.reconnect_due = None;
        m.reconnect_skipped();
        assert_eq!(m.dto.reconnect.unwrap().attempt, 2);
        assert!(m.reconnect_due.unwrap().delay >= Duration::from_secs(3));
    }

    #[test]
    fn a_full_account_is_said_and_not_reconnected() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.playing();
        network_failure(&mut m);
        m.in_use(connections::Slots { max: 1, active: 1 });
        let e = m.dto.ending.unwrap();
        assert_eq!(e.kind, EndingKind::InUse);
        assert_eq!(e.slots, Some(connections::Slots { max: 1, active: 1 }));
        assert!(m.dto.reconnect.is_none());
        assert_eq!(m.dto.state, PlaybackState::Failed);
    }

    #[test]
    fn a_picture_ends_the_reconnecting() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.reconnects = 1;
        m.dto.reconnect = Some(ReconnectDto { attempt: 1, of: 4, checking: false });
        m.playing();
        assert!(m.dto.reconnect.is_none());
        assert!(m.reconnected_at.is_some());
    }

    #[test]
    fn a_zap_or_a_stop_makes_a_waiting_reconnect_stale() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.playing();
        network_failure(&mut m);
        let token = m.reconnect_token;
        m.opening(Opened::Channel(8));
        assert_ne!(m.reconnect_token, token);
        assert!(m.dto.reconnect.is_none());
        let token = m.reconnect_token;
        m.stopped();
        assert_ne!(m.reconnect_token, token);
    }

    #[test]
    fn a_stall_is_reconnected_but_a_takeover_is_not() {
        // Data for ten seconds, then quiet until mpv's timeout reads the end.
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.playing();
        for i in 1..=72 {
            let t = f64::from(i) * 0.25;
            m.flow.observe(0.25, Some(t.min(10.0)), t >= 18.0, Some(1_000_000));
        }
        m.ended();
        assert_eq!(m.dto.ending.unwrap().kind, EndingKind::Network);
        assert!(m.reconnect_due.is_some());

        // The provider closing it: the end is read the moment data stops.
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.playing();
        for i in 1..=72 {
            let t = f64::from(i) * 0.25;
            m.flow.observe(0.25, Some(t.min(10.0)), t >= 10.25, Some(1_000_000));
        }
        m.ended();
        assert_eq!(m.dto.ending.unwrap().kind, EndingKind::Dropped);
        assert!(m.reconnect_due.is_none());
    }

    #[test]
    fn quality_is_not_taken_before_a_frame_exists() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
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
        m.opening(Opened::Channel(7));
        assert!(m.changed().is_some());
    }

    #[test]
    fn a_replaced_file_is_not_the_new_streams_end() {
        // Guards the zap race: mpv reports the *previous* file ending after
        // the next one has already been asked for.
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
        m.playing();
        m.opening(Opened::Channel(2));
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
        m.opening(Opened::Channel(42));
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
        m.opening(Opened::Channel(42));
        m.live = false;
        m.playing();
        m.ended();
        assert_eq!(m.dto.state, PlaybackState::Stopped);
        assert!(m.dto.ending.is_none());
    }

    #[test]
    fn a_refusal_carries_the_status_mpv_logged() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(42));
        m.clues.observe("http: HTTP error 509 Max Connections Reached");
        m.ended_with(How::Error(-13));
        assert_eq!(m.dto.state, PlaybackState::Failed);
        assert_eq!(m.dto.error.as_deref(), Some("the stream could not be opened"));
        let e = m.dto.ending.unwrap();
        assert_eq!((e.kind, e.status, e.limit), (ending::EndingKind::Refused, Some(509), true));
    }

    /// mpv's cache as a placeholder clip leaves it: ten minutes of stream,
    /// all of it in (6.6 MB, as the fixture), and the end read.
    fn clip_read(eof: bool) -> Probe {
        Probe {
            cache_start: Some(1.4),
            cache_end: Some(601.4),
            time_pos: Some(1.4),
            behind_live: 600.0,
            demuxer_eof: eof,
            cache_bytes: Some(6_600_000),
            paused: true,
            ..Probe::default()
        }
    }

    const TS: Option<&str> = Some("mpegts");

    /// This open's own file has started: the watch reads it.
    fn armed(m: &mut Machine) {
        m.file_started();
        m.arm(true);
    }

    /// Long enough after an open for the placeholder watch to be over.
    fn settled() -> Instant {
        Instant::now() + Duration::from_secs_f64(ending::OFF_AIR_WINDOW + 1.0)
    }

    /// Heartbeats of a placeholder clip arriving over three seconds, then
    /// its end, in `format`. Returns what the machine decided on the last.
    fn a_clip_arrives_as(m: &mut Machine, format: Option<&str>) -> Option<f64> {
        for i in 1..12 {
            let p = Probe {
                cache_end: Some(1.4 + 50.0 * f64::from(i)),
                demuxer_eof: false,
                ..clip_read(false)
            };
            assert_eq!(m.watch_for_placeholder(0.25, &p, None), None, "nothing until the end is read");
        }
        m.watch_for_placeholder(0.25, &clip_read(true), format)
    }

    fn a_clip_arrives(m: &mut Machine) -> Option<f64> {
        a_clip_arrives_as(m, TS)
    }

    /// A live channel opened and its file started.
    fn live(id: i64, mode: LiveStart) -> Machine {
        let mut m = Machine::new();
        m.opening(Opened::Channel(id));
        m.starting(mode);
        armed(&mut m);
        m
    }

    #[test]
    fn a_placeholder_on_a_live_channel_is_off_air_and_stops_there() {
        for mode in [LiveStart::CatchUp, LiveStart::Stay, LiveStart::Wait] {
            let mut m = live(7, mode);
            m.playing();
            assert_eq!(a_clip_arrives(&mut m), Some(600.0), "{mode:?}");
            assert_eq!(m.dto.state, PlaybackState::Failed, "{mode:?}");
            let e = m.dto.ending.unwrap();
            assert_eq!((e.kind, e.clip_secs), (EndingKind::OffAir, Some(600)), "{mode:?}");
            // Never by itself: no reconnect, nothing following the edge.
            assert!(m.reconnect_due.is_none() && m.dto.reconnect.is_none(), "{mode:?}");
            assert!(!m.following_live, "{mode:?}");
            assert_eq!(m.dto.stream_id, Some(7), "Retry knows what to tune");
        }
    }

    #[test]
    fn a_stale_probe_from_the_last_file_decides_nothing() {
        // A movie (or a programme, or a stream playing out its buffer) near
        // its end: ten minutes in the cache and its end read. A live channel
        // opens; until mpv has started this open's file, what it reports is
        // still the last one's.
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.starting(LiveStart::CatchUp);
        for _ in 0..8 {
            assert_eq!(m.watch_for_placeholder(0.25, &clip_read(true), TS), None);
        }
        // A file started, but mpv's path is not this open's: the last
        // file's start arriving late.
        m.file_started();
        m.arm(false);
        assert_eq!(m.watch_for_placeholder(0.25, &clip_read(true), TS), None);
        // An address match with no start since the open is not enough
        // either: a Retry of the same channel loads the same address.
        let mut r = Machine::new();
        r.opening(Opened::Channel(7));
        r.arm(true);
        assert_eq!(r.watch_for_placeholder(0.25, &clip_read(true), TS), None);
        // Its first frame then plays as any other.
        m.playing();
        assert_eq!(m.dto.state, PlaybackState::Playing);
        // And once this open's file is the one read, the watch is on.
        m.arm(true);
        assert_eq!(a_clip_arrives(&mut m), Some(600.0));
    }

    #[test]
    fn an_episode_in_a_file_container_is_not_off_air() {
        // A playlist's episode, listed among the channels: an MP4 sent fast
        // and then ended. It plays.
        for format in [Some("mov,mp4,m4a,3gp,3g2,mj2"), Some("matroska,webm"), None] {
            let mut m = live(7, LiveStart::CatchUp);
            m.playing();
            assert_eq!(a_clip_arrives_as(&mut m, format), None, "{format:?}");
            assert_eq!(m.dto.state, PlaybackState::Playing, "{format:?}");
        }
    }

    #[test]
    fn the_network_failing_first_is_not_off_air() {
        let mut m = live(7, LiveStart::CatchUp);
        m.playing();
        m.clues.observe("Connection reset by peer");
        assert_eq!(a_clip_arrives(&mut m), None);
    }

    #[test]
    fn the_hold_lets_go_without_skipping_when_it_is_off_air() {
        // Start at live holds the first frame and would skip to the newest
        // data once it stopped arriving: the clip's last second. The
        // decision comes first, and the hold is over with no seek.
        let mut m = live(7, LiveStart::Wait);
        m.playing();
        assert!(m.join.is_some());
        assert!(a_clip_arrives(&mut m).is_some());
        assert!(m.join.is_none() && !m.joining && !m.dto.joining_live);
        assert!(m.settle.is_none(), "nothing left to catch up either");
    }

    #[test]
    fn what_comes_after_the_stop_is_not_a_second_ending() {
        let mut m = live(7, LiveStart::CatchUp);
        m.playing();
        a_clip_arrives(&mut m);
        // mpv's own end-file, had it played out what it had, would not
        // replace the explanation; nor would a late first frame.
        m.ended();
        m.playing();
        assert_eq!(m.dto.state, PlaybackState::Failed);
        assert_eq!(m.dto.ending.unwrap().kind, EndingKind::OffAir);
        assert_eq!(m.watch_for_placeholder(0.25, &clip_read(true), TS), None);
    }

    #[test]
    fn a_normal_burst_then_an_end_ends_as_it_did() {
        // A minute of connect burst, then the provider ends it five seconds
        // in (another device), or the network fails.
        let burst = |eof| Probe {
            cache_start: Some(0.0),
            cache_end: Some(65.0),
            time_pos: Some(0.0),
            demuxer_eof: eof,
            cache_bytes: Some(65 * 750_000),
            ..Probe::default()
        };
        let mut m = live(7, LiveStart::CatchUp);
        m.playing();
        assert_eq!(m.watch_for_placeholder(5.0, &burst(true), TS), None);
        assert_eq!(m.dto.state, PlaybackState::Playing, "it plays out what it has");
        m.ended();
        assert_eq!(m.dto.ending.unwrap().kind, EndingKind::Dropped);

        let mut m = live(7, LiveStart::CatchUp);
        m.playing();
        assert_eq!(m.watch_for_placeholder(5.0, &burst(true), TS), None);
        network_failure(&mut m);
        assert_eq!(m.dto.ending.unwrap().kind, EndingKind::Network);
        assert!(m.reconnect_due.is_some(), "and is reconnected as before");
    }

    #[test]
    fn an_end_read_after_the_window_ends_as_it_did() {
        let mut m = live(7, LiveStart::CatchUp);
        m.playing();
        let late = ending::OFF_AIR_WINDOW + 1.0;
        assert_eq!(m.watch_for_placeholder(late, &clip_read(true), TS), None);
        assert_eq!(m.dto.state, PlaybackState::Playing);
        m.ended();
        assert_eq!(m.dto.ending.unwrap().kind, EndingKind::Dropped);
    }

    #[test]
    fn without_the_cache_state_it_ends_as_it_did() {
        let mut m = live(7, LiveStart::CatchUp);
        m.playing();
        // `demuxer-cache-state` unreadable: no end, no start, no span.
        let blind = Probe { time_pos: Some(3.0), ..Probe::default() };
        for _ in 0..80 {
            assert_eq!(m.watch_for_placeholder(0.25, &blind, None), None);
        }
        m.ended();
        assert_eq!(m.dto.ending.unwrap().kind, EndingKind::Dropped);
    }

    #[test]
    fn a_catch_up_programme_a_movie_or_an_episode_is_never_off_air() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.live = false;
        m.starting(LiveStart::CatchUp);
        armed(&mut m);
        m.playing();
        assert_eq!(a_clip_arrives(&mut m), None);
        assert_eq!(m.dto.state, PlaybackState::Playing);

        let mut m = a_playing_movie(3);
        armed(&mut m);
        assert_eq!(a_clip_arrives(&mut m), None);
        assert_eq!(m.dto.state, PlaybackState::Playing);

        let mut m = a_playing_episode(3);
        armed(&mut m);
        assert_eq!(a_clip_arrives(&mut m), None);
        assert_eq!(m.dto.state, PlaybackState::Playing);
    }

    /// One heartbeat with ten minutes of stream already in and no end yet:
    /// a clip-shaped buffer.
    fn a_clip_is_in(m: &mut Machine) {
        assert_eq!(m.watch_for_placeholder(0.25, &clip_read(false), None), None);
    }

    #[test]
    fn off_air_is_written_against_the_channel_and_its_frame_never_as_a_load() {
        let mut m = live(7, LiveStart::CatchUp);
        m.playing();
        a_clip_is_in(&mut m);
        let t0 = Instant::now();
        assert_eq!(m.ledger(t0), None, "a clip-shaped buffer: the first frame may be the clip's");
        assert_eq!(m.ledger(t0 + MEASURE_AFTER), None, "nor its black picture measured");
        a_clip_arrives(&mut m);
        assert_eq!(m.ledger(t0), Some(Ledger::Failed(7, "off air".into())));
        assert_eq!(m.ledger(settled()), None, "once per open, and no load after it");
        // The next live play clears it, as any other failure.
        let mut m = live(7, LiveStart::CatchUp);
        m.playing();
        assert_eq!(m.ledger(Instant::now()), Some(Ledger::Loaded(7)));
    }

    #[test]
    fn off_air_found_before_the_first_frame_is_not_written_as_loaded() {
        let mut m = live(7, LiveStart::CatchUp);
        a_clip_arrives(&mut m);
        m.playing();
        assert_eq!(m.ledger(Instant::now()), Some(Ledger::Failed(7, "off air".into())));
        assert_eq!(m.ledger(settled()), None);
    }

    #[test]
    fn a_normal_live_channel_writes_as_it_always_did() {
        // A 20 s connect burst, then live: never clip-shaped, never held.
        let mut m = live(7, LiveStart::CatchUp);
        let burst = Probe {
            cache_start: Some(0.0),
            cache_end: Some(20.0),
            time_pos: Some(0.0),
            cache_bytes: Some(20 * 750_000),
            ..Probe::default()
        };
        assert_eq!(m.watch_for_placeholder(0.25, &burst, None), None);
        m.playing();
        let t0 = Instant::now();
        assert_eq!(m.ledger(t0), Some(Ledger::Loaded(7)), "at the first frame");
        assert_eq!(m.ledger(t0 + MEASURE_AFTER / 2), None);
        let at = Instant::now() + MEASURE_AFTER;
        assert_eq!(m.ledger(at), Some(Ledger::Measure(7)), "at MEASURE_AFTER");
        assert_eq!(m.ledger(at), None);
        // The same before the watch is armed, or with no cache state at all.
        let mut m = Machine::new();
        m.opening(Opened::Channel(8));
        m.playing();
        assert_eq!(m.ledger(Instant::now()), Some(Ledger::Loaded(8)));
    }

    #[test]
    fn a_load_waits_until_the_end_is_judged_not_a_placeholder() {
        // A clip-shaped buffer that ends as something else (an episode in
        // MP4): judged, and the load written at once.
        let mut m = live(7, LiveStart::CatchUp);
        m.playing();
        a_clip_is_in(&mut m);
        assert_eq!(m.ledger(Instant::now()), None);
        assert_eq!(m.watch_for_placeholder(0.25, &clip_read(true), Some("mov,mp4,m4a,3gp,3g2,mj2")), None);
        assert_eq!(m.ledger(Instant::now()), Some(Ledger::Loaded(7)));
        // Or the window passes with no end: written then.
        let mut m = live(8, LiveStart::CatchUp);
        m.playing();
        a_clip_is_in(&mut m);
        assert_eq!(m.ledger(Instant::now()), None);
        assert_eq!(m.ledger(settled()), Some(Ledger::Loaded(8)));
    }

    #[test]
    fn a_load_held_back_is_written_when_the_channel_is_left_early() {
        // A picture, held, then a zap within the window: the load stands.
        let mut m = live(7, LiveStart::CatchUp);
        m.playing();
        a_clip_is_in(&mut m);
        assert_eq!(m.ledger(Instant::now()), None);
        m.opening(Opened::Channel(8));
        assert_eq!(m.ledger(Instant::now()), Some(Ledger::Loaded(7)));
        assert_eq!(m.ledger(Instant::now()), None, "and the new one has shown nothing yet");
        // Stopping, the same.
        armed(&mut m);
        m.playing();
        a_clip_is_in(&mut m);
        assert_eq!(m.ledger(Instant::now()), None);
        m.stopped();
        assert_eq!(m.ledger(Instant::now()), Some(Ledger::Loaded(8)));
        // A provider ending it within the window: written then.
        let mut m = live(9, LiveStart::CatchUp);
        m.playing();
        a_clip_is_in(&mut m);
        m.ended();
        assert_eq!(m.ledger(Instant::now()), Some(Ledger::Loaded(9)));
        // An off-air channel left early has nothing to carry.
        let mut m = live(10, LiveStart::CatchUp);
        m.playing();
        a_clip_arrives(&mut m);
        assert_eq!(m.ledger(Instant::now()), Some(Ledger::Failed(10, "off air".into())));
        m.opening(Opened::Channel(11));
        assert_eq!(m.ledger(Instant::now()), None);
    }

    #[test]
    fn a_channel_held_for_live_is_measured_once_it_moves() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        m.joining = true;
        let t0 = Instant::now();
        m.playing();
        assert_eq!(m.ledger(t0), Some(Ledger::Loaded(7)), "the held frame is the open");
        m.joined();
        assert_eq!(m.ledger(Instant::now() + MEASURE_AFTER / 2), None);
        let later = Instant::now() + MEASURE_AFTER;
        assert_eq!(m.ledger(later), Some(Ledger::Measure(7)));
        assert_eq!(m.ledger(later), None, "measured once per open");
    }

    #[test]
    fn a_channel_that_loads_is_noted_once_then_measured_after_a_while() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(7));
        let t0 = Instant::now();
        assert_eq!(m.ledger(t0), None, "opening is not loading");
        m.playing();
        assert_eq!(m.ledger(t0), Some(Ledger::Loaded(7)));
        assert_eq!(m.ledger(t0), None, "once per open");
        assert_eq!(m.ledger(Instant::now() + MEASURE_AFTER / 2), None);
        let later = Instant::now() + MEASURE_AFTER;
        assert_eq!(m.ledger(later), Some(Ledger::Measure(7)));
        assert_eq!(m.ledger(later), None, "measured once per open");
        // A resolution change later in the stream is not a second open.
        m.playing();
        assert_eq!(m.ledger(later + MEASURE_AFTER), None);
    }

    #[test]
    fn a_genuine_failure_to_load_is_noted_with_its_reason() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(9));
        m.clues.observe("http: HTTP error 404 Not Found");
        m.ended_with(How::Error(-13));
        assert_eq!(
            m.ledger(Instant::now()),
            Some(Ledger::Failed(9, "not on the server (HTTP 404)".into()))
        );
        assert_eq!(m.ledger(Instant::now()), None, "once per open");
        // Retrying is a new open, with its own outcome.
        m.opening(Opened::Channel(9));
        m.playing();
        assert_eq!(m.ledger(settled()), Some(Ledger::Loaded(9)));
    }

    #[test]
    fn the_account_limit_and_the_network_are_not_held_against_a_channel() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
        m.clues.observe("http: HTTP error 509 Max Connections Reached");
        m.ended_with(How::Error(-13));
        assert_eq!(m.ledger(Instant::now()), None);
        m.opening(Opened::Channel(2));
        m.clues.observe("tcp: Connection to tcp://10.0.0.1:80 failed: Connection timed out");
        m.ended_with(How::Error(-13));
        assert_eq!(m.ledger(Instant::now()), None);
    }

    #[test]
    fn a_bare_deadline_waits_for_mpvs_reason() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(3));
        m.ended_with(How::Deadline);
        assert_eq!(m.ledger(Instant::now()), None, "mpv may yet say why");
        // mpv's reason arrives, and it is the network: nothing to hold
        // against the channel.
        m.clues.observe("Connection timed out");
        m.ended_with(How::Error(-13));
        assert_eq!(m.ledger(Instant::now() + FAILURE_SETTLE), None);

        // Without a reason, the deadline stands once mpv has had its time.
        m.opening(Opened::Channel(4));
        m.ended_with(How::Deadline);
        assert_eq!(
            m.ledger(Instant::now() + FAILURE_SETTLE),
            Some(Ledger::Failed(4, "no picture arrived".into()))
        );
    }

    #[test]
    fn a_stream_dropped_mid_play_or_a_catch_up_says_nothing_about_the_channel() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(5));
        m.playing();
        m.ended();
        // The picture it showed counts; the ending does not.
        assert_eq!(m.ledger(Instant::now()), Some(Ledger::Loaded(5)));
        assert_eq!(m.ledger(Instant::now() + FAILURE_SETTLE), None);

        m.opening(Opened::Channel(6));
        m.live = false;
        m.ended_with(How::Error(-13));
        assert_eq!(m.ledger(Instant::now() + FAILURE_SETTLE), None);
        m.opening(Opened::Channel(6));
        m.live = false;
        m.playing();
        assert_eq!(m.ledger(Instant::now() + MEASURE_AFTER), None);
    }

    #[test]
    fn a_new_channel_forgets_the_last_ones_ending() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
        m.clues.observe("HTTP error 403 Forbidden");
        m.ended_with(How::Error(-13));
        m.opening(Opened::Channel(2));
        assert!(m.dto.ending.is_none());
        assert_eq!(m.clues, Clues::default(), "the evidence was about the other stream");
        m.playing();
        m.ended();
        assert_eq!(m.dto.ending.unwrap().status, None);
    }

    #[test]
    fn mpvs_reason_refines_the_deadline_but_nothing_else_is_overwritten() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
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
    fn mpvs_words_are_read_before_the_ending_they_explain() {
        // mpv hands out queued events before the log lines waiting beside
        // them, so a refusal pumped in one go arrives as its end, then why.
        let batch = vec![
            Signal::StartFile,
            Signal::EndFile { error: Some(-13), replaced: false },
            Signal::Log("http: HTTP error 404 Not Found".into()),
            Signal::Log("Failed to open the stream.".into()),
        ];
        let ordered: Vec<&Signal> = logs_first(&batch);
        assert_eq!(
            ordered,
            [&batch[2], &batch[3], &batch[0], &batch[1]],
            "the words first, in their order, then the events in theirs"
        );
        let mut m = catchup(42, LiveStart::CatchUp);
        for s in ordered {
            match s {
                Signal::Log(text) => m.clues.observe(text),
                Signal::EndFile { error: Some(code), .. } => m.ended_with(How::Error(*code)),
                _ => {}
            }
        }
        let e = m.dto.ending.unwrap();
        assert_eq!((e.kind, e.status), (EndingKind::BeyondArchive, Some(404)));
    }

    #[test]
    fn the_last_streams_picture_going_away_does_not_revive_a_failed_open() {
        // A refusal from a server on the LAN arrives before mpv has taken
        // the previous stream's picture down, and that teardown is a video
        // reconfigure too. It is no frame of this stream.
        let mut m = catchup(42, LiveStart::CatchUp);
        m.clues.observe("http: HTTP error 404 Not Found");
        m.ended_with(How::Error(-13));
        m.playing();
        assert_eq!(m.dto.state, PlaybackState::Failed);
        assert_eq!(m.dto.ending.map(|e| e.kind), Some(EndingKind::BeyondArchive));
        // Nor before it failed: the teardown, pumped after the next open
        // began, is no picture of it.
        let mut m = catchup(42, LiveStart::CatchUp);
        assert!(!m.reconfigured(false));
        assert_eq!(m.dto.state, PlaybackState::Opening);
        m.clues.observe("http: HTTP error 404 Not Found");
        m.ended_with(How::Error(-13));
        assert_eq!(m.dto.ending.map(|e| e.kind), Some(EndingKind::BeyondArchive));
        // A reconfigure with a picture is the first frame.
        let mut m = catchup(42, LiveStart::CatchUp);
        assert!(m.reconfigured(true));
        assert_eq!(m.dto.state, PlaybackState::Playing);
        // Nor an open that could not be sent.
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
        m.failed("could not open");
        m.playing();
        assert_eq!(m.dto.state, PlaybackState::Failed);
        // A stream that was only slow — failed on the deadline, still
        // loading — is playing when its first frame comes.
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
        m.ended_with(How::Deadline);
        m.playing();
        assert_eq!(m.dto.state, PlaybackState::Playing);
    }

    #[test]
    fn stopping_clears_the_explanation() {
        let mut m = Machine::new();
        m.opening(Opened::Channel(1));
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
    fn timeshift_stamp_is_the_servers_wall_clock_not_the_viewers() {
        // As measured: the server an hour east of UTC, the viewer five
        // hours west, at 20:17:17 UTC. "Five minutes ago" is asked for from
        // 20:12 UTC (the scrubber rounds down to the minute).
        let now = 1_794_082_637;
        let five_ago = (now - 300) / 60 * 60;
        let server = timeshift_stamp(five_ago, 3600);
        assert_eq!(server, "2026-11-07:21-12");
        // The viewer's wall clock, which the server reads as its own: the
        // same digits mean six hours earlier there.
        let viewer = timeshift_stamp(five_ago, -5 * 3600);
        assert_eq!(viewer, "2026-11-07:15-12");
        // Read back in the server's zone, the right stamp is the right
        // moment, and the wrong one is six hours off.
        let read_back = |stamp: &str| {
            let (date, hm) = stamp.split_once(':').unwrap();
            let mut d = date.split('-').map(|p| p.parse::<i64>().unwrap());
            let (y, m, dd) = (d.next().unwrap(), d.next().unwrap(), d.next().unwrap());
            let (h, mi) = hm.split_once('-').unwrap();
            crate::localtime::days_from_civil(y, m as u32, dd as u32) * 86_400
                + h.parse::<i64>().unwrap() * 3600
                + mi.parse::<i64>().unwrap() * 60
                - 3600
        };
        assert_eq!(read_back(&server), five_ago);
        assert_eq!(five_ago - read_back(&viewer), 6 * 3600);
        // Half and quarter hours east of UTC.
        assert_eq!(timeshift_stamp(five_ago, 5 * 3600 + 1800), "2026-11-08:01-42");
        assert_eq!(timeshift_stamp(five_ago, 9 * 3600 + 2700), "2026-11-08:05-57");
    }

    #[test]
    fn a_request_older_than_the_archive_keeps_starts_inside_it() {
        let now = 1_794_082_637;
        let earliest = crate::catalog::guide_search::earliest_catchup(now, 3);
        assert_eq!(earliest, now - 3 * 86_400 + 600);
        // At the very edge: moved ten minutes in, to the next whole minute,
        // still ending where it would have.
        let edge = (now - 3 * 86_400) / 60 * 60;
        let (start, minutes) = clamp_to_archive(edge, 60, Some(earliest));
        assert!(start >= earliest && start - earliest < 60, "{start} vs {earliest}");
        assert_eq!(start % 60, 0);
        assert_eq!(start + i64::from(minutes) * 60, edge + 60 * 60);
        // Inside the margin already, or of unknown depth: left alone.
        assert_eq!(clamp_to_archive(earliest + 60, 30, Some(earliest)), (earliest + 60, 30));
        assert_eq!(clamp_to_archive(edge, 30, None), (edge, 30));
        // A request that ended before the margin still asks for a minute.
        let (start, minutes) = clamp_to_archive(edge - 3600, 5, Some(earliest));
        assert!(start >= earliest);
        assert_eq!(minutes, 1);
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
            let off = crate::localtime::utc_offset(t);
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
    LIVE_START.store(settings.live_start as u8, Ordering::Release);
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
    LIVE_START.store(saved.live_start as u8, Ordering::Release);
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
