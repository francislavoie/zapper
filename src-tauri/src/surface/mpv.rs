//! libmpv: the one player, its tuned options, its event queue, and the
//! properties the interface reads.
//!
//! Portable. How the picture reaches the screen is the platform's business
//! and enters here in exactly two places: the `video_out` options handed to
//! [`Player::new`] (Linux asks for `vo=libmpv` and renders through GL itself;
//! Windows gives mpv a window to draw into), and the readiness gate in
//! [`Player::load`].
//!
//! Main-thread-only by convention: commands reach the player through
//! `AppHandle::run_on_main_thread` and [`with_player`], which answers `None`
//! anywhere else. The mpv handle itself is thread-safe, and is shared with
//! [`handle`] only for work that must not block the main thread.

use anyhow::{anyhow, Context, Result};
use libmpv2::Mpv;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// The process's mpv. Leaked deliberately: Linux's render context borrows it
/// for `'static`, and the player outlives everything anyway, so a
/// self-referential struct is avoided. `Mpv` is `Send + Sync` (libmpv's
/// client API is thread-safe), which is what lets it be a plain static.
static MPV: OnceLock<&'static Mpv> = OnceLock::new();

thread_local! {
    /// A URL asked for before the surface could show it. See [`Player::load`].
    static PENDING: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// FFmpeg's HTTP options that would open a stream again by themselves.
///
/// mpv turns `reconnect` on for every HTTP stream, and FFmpeg then quietly
/// reconnects when a seekable response — a catch-up programme, say — is cut
/// off. When the provider cut it off because another device took the
/// account's connection, that reconnect ends the other device's stream in
/// turn, and the two take it from each other until somebody gives up. A
/// stream the provider ended stays ended; the interface says so and offers
/// Retry (see `ipc::ending`). The other two are off by default in FFmpeg and
/// are named so that stays true whatever a build's defaults are.
///
/// Measured against a server that cuts a seekable response off a third of
/// the way through: mpv's defaults made 80 requests before giving up; with
/// these, 3 — the open, and the two seeks any seekable file costs.
///
/// `ytdl` is switched off in [`Player::new`] for the same reason, and it was
/// the worse of the two. mpv's built-in youtube-dl hook runs whenever a
/// stream fails to open, and where `yt-dlp` is installed it asks the server
/// for the same URL again, as a browser — so every refused open was two
/// requests, the second a fresh connection to a provider that had just said
/// no. Measured with `ending_check`, which counts them. Live TV has no use
/// for the hook in the first place.
pub const NO_RECONNECT: &str = "reconnect=0,reconnect_streamed=0,reconnect_at_eof=0";

/// Asks mpv for its warnings and errors as events.
///
/// The only place mpv reports the HTTP status that refused a stream is its
/// log — FFmpeg's `HTTP error 509 Max Connections Reached` — and the
/// `end-file` that follows carries nothing but "loading failed". Warnings
/// rather than everything: the lines that matter are warnings, and below
/// that the decoder chatters on every frame. Not exposed by the safe
/// wrapper, so through libmpv2-sys.
pub fn request_warnings(mpv: &Mpv) {
    // SAFETY: the handle is live for the call, and the level is a static
    // NUL-terminated string mpv copies.
    let rc = unsafe { libmpv2_sys::mpv_request_log_messages(mpv.ctx.as_ptr(), c"warn".as_ptr()) };
    if rc < 0 {
        tracing::warn!("mpv will not report its warnings ({rc}); stream refusals will be less specific");
    }
}

/// How far ahead of the playhead mpv may buffer, in bytes.
pub const DEMUXER_MAX_BYTES: i64 = 256 * 1024 * 1024;

/// A handle to the process-wide mpv instance.
///
/// Zero-sized: the real state is the static above, plus whatever the
/// platform's surface keeps in its own main-thread locals.
pub struct Player;

impl Player {
    /// Creates the mpv handle with zapper's options, then `video_out`'s.
    ///
    /// `video_out` is the platform's say in how video is presented — the
    /// `vo`, and whatever goes with it — applied after the shared options so
    /// that a platform can override one of them where it must. Called by the
    /// platform's surface, never by command code.
    pub fn new(
        video_out: impl FnOnce(&libmpv2::MpvInitializer) -> libmpv2::Result<()>,
    ) -> Result<Self> {
        let mpv = Mpv::with_initializer(|init| {
            // Live rewind: ~512 MiB, roughly 3.7 minutes at 18 Mbps.
            init.set_property("cache", "yes".to_owned())?;
            init.set_property("demuxer-max-back-bytes", 512i64 * 1024 * 1024)?;
            init.set_property("demuxer-max-bytes", DEMUXER_MAX_BYTES)?;
            init.set_property("hwdec", "auto-safe".to_owned())?;
            // Exact seeks, and the reason is measured rather than stylistic.
            //
            // mpv's default seeks to the nearest preceding keyframe, and on a
            // broadcast MPEG-TS those are seconds apart. Benchmarked against
            // the loop server (`cargo run --bin seek_bench`), a -5s request
            // under the default actually moved -7.8s, -6.9s and -6.9s, and
            // twice landed on 0.00 - the very start of the buffer, roughly 19
            // seconds behind live, which then has to refill. That is what
            // "rewind feels slow" was: not the seek, the place it landed.
            //
            // Exact seeking lands on -5.00 every time and costs 83ms against
            // the default's 35ms. Both are far below anything a viewer can
            // perceive, and the accuracy is the whole point of a skip button.
            init.set_property("hr-seek", "yes".to_owned())?;
            // Deinterlace what says it is interlaced, and nothing else.
            //
            // Plenty of live channels are still 1080i or 576i, and shown
            // without this they comb on every movement. `auto` costs nothing
            // on the progressive majority: mpv reads each frame's interlaced
            // flag and, without it, builds no filter at all (filters/
            // f_auto_filters.c). Measured with mpv 0.41, software decode,
            // eight seconds of each fixture: progressive 1080p25 used 1.77s
            // of CPU under `auto` and 1.78s under `no` (720p60: 1.82s and
            // 1.86s), where `yes` costs 4.06s by filtering frames that never
            // needed it. On a flagged 1080i clip `auto` engages exactly as
            // `yes` does (2.94s and 2.96s). Hardware frames take the same
            // per-frame test before mpv picks the GPU's own deinterlacer
            // (`vavpp` on VA-API, `d3d11vpp` on Windows).
            init.set_property("deinterlace", "auto".to_owned())?;
            // HDR on an SDR panel must be tone-mapped, not washed out.
            init.set_property("tone-mapping", "auto".to_owned())?;
            init.set_property("target-colorspace-hint", "yes".to_owned())?;
            init.set_property("volume-max", 200i64)?;
            // Some servers reject unknown user agents; VLC's is widely
            // accepted.
            init.set_property("user-agent", "VLC/3.0.20 LibVLC/3.0.20".to_owned())?;
            init.set_property("network-timeout", 8i64)?;
            init.set_property("stream-lavf-o", NO_RECONNECT.to_owned())?;
            // No youtube-dl lookups on a stream URL. The option exists only
            // where mpv has Lua, which runs the hook; zapper's own libmpv is
            // built without either, so there it is already off and setting
            // it fails.
            let _ = init.set_property("ytdl", false);
            video_out(&init)?;
            Ok(())
        })
        .context("initialising mpv")?;
        request_warnings(&mpv);

        // The track list is pushed rather than polled: it changes once per
        // channel, or when a multiplex announces a track late, and also when
        // a track is switched (its `selected` flags move). Observed as a
        // string, mpv hands over the whole list as JSON with each change.
        // Not fatal: without it the picture menu offers no track choice.
        if let Err(e) = mpv.observe_property("track-list", libmpv2::Format::String, 0) {
            tracing::warn!("could not observe the track list: {e}");
        }

        let mpv: &'static Mpv = Box::leak(Box::new(mpv));
        MPV.set(mpv)
            .map_err(|_| anyhow!("mpv is already initialised in this process"))?;
        // Register on the way out so command code can reach the player without
        // `lib.rs` having to remember a second call.
        set_installed(Player);
        Ok(Self)
    }

    fn mpv() -> Result<&'static Mpv> {
        handle().ok_or_else(|| anyhow!("mpv is not initialised"))
    }

    /// Starts playback, deferring if the surface is not ready yet.
    ///
    /// mpv initialises its video output as soon as a file opens. On Linux,
    /// if that happens before the render context exists, the VO fails
    /// outright - "Error opening/initializing the selected video_out (--vo)
    /// device" - and nothing ever decodes. A network stream hides this by
    /// taking a second to open, which is long enough for the first frame to
    /// have been drawn; a local file opens instantly and fails every time. So
    /// a load requested before the surface is ready is held until it is (see
    /// [`load_pending`]). Windows hands mpv a window up front and is always
    /// ready.
    pub fn load(&self, url: &str) -> Result<()> {
        if !crate::surface::platform::video_ready() {
            PENDING.with(|p| *p.borrow_mut() = Some(url.to_owned()));
            crate::surface::platform::request_frame();
            return Ok(());
        }
        Self::mpv()?
            .command("loadfile", &[url, "replace"])
            .context("loadfile")
    }

    pub fn set_property(&self, key: &str, value: &str) -> Result<()> {
        Self::mpv()?
            .set_property(key, value.to_owned())
            .with_context(|| format!("setting {key}"))
    }

    /// The decoded video's pixel dimensions, or `None` before the first frame.
    pub fn dimensions(&self) -> Option<(i64, i64)> {
        let m = Self::mpv().ok()?;
        Some((
            m.get_property::<i64>("width").ok()?,
            m.get_property::<i64>("height").ok()?,
        ))
    }
}

/// The mpv handle, for the platform's surface: Linux's render context is
/// created from it, and Windows takes thumbnails through it off the main
/// thread. `None` before [`Player::new`].
pub fn handle() -> Option<&'static Mpv> {
    MPV.get().copied()
}

/// What the demuxer cache holds, in the stream's own time: from the oldest
/// moment that can be read back to the newest packet. `None` before a stream
/// has data.
pub fn cache_span(mpv: &Mpv) -> Option<(f64, f64)> {
    let raw = mpv.get_property::<String>("demuxer-cache-state").ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let start = v
        .get("seekable-ranges")
        .and_then(|r| r.as_array())
        .and_then(|r| r.first())
        .and_then(|r| r.get("start"))
        .and_then(|x| x.as_f64())?;
    let end = v.get("cache-end").and_then(|x| x.as_f64())?;
    (end > start).then_some((start, end))
}

/// Writes what the cache holds from `start` to `end` (the stream's time, as
/// [`cache_span`] gives it) to `path`, as MPEG-TS: mpv's `dump-cache`, the
/// packets copied as they are. Blocks until the file is written, so never on
/// the main thread; playback carries on meanwhile.
pub fn dump_cache(mpv: &Mpv, start: f64, end: f64, path: &std::path::Path) -> Result<()> {
    let path = path
        .to_str()
        .ok_or_else(|| anyhow!("a path mpv cannot be given"))?;
    mpv.command(
        "dump-cache",
        &[&format!("{start:.3}"), &format!("{end:.3}"), path],
    )
    .context("dump-cache")
}

/// Opens the URL held back by [`Player::load`], now that the surface can show
/// it. Called by the platform once its video output can succeed.
pub fn load_pending() {
    let Some(url) = PENDING.with(|p| p.borrow_mut().take()) else {
        return;
    };
    let Some(mpv) = handle() else { return };
    if let Err(e) = mpv.command("loadfile", &[&url, "replace"]) {
        tracing::error!("deferred loadfile: {e}");
    }
}

// ---------------------------------------------------------------------------
// Command-facing API: registry, event pump, and property probe.
//
// Everything below is main-thread-only: the registry is a thread-local of the
// main thread, so the player is reachable only there. Tauri commands reach it
// through `AppHandle::run_on_main_thread` and [`with_player`].
// ---------------------------------------------------------------------------

thread_local! {
    /// The installed player, reachable only from the main thread.
    ///
    /// `Player` is zero-sized, but the registry still has to exist: the
    /// surface it draws into is main-thread state on every platform, so "is
    /// there a player here" is a per-thread question.
    static INSTALLED: RefCell<Option<Player>> = const { RefCell::new(None) };

    /// One ticker per process. A second one would double every state update
    /// and drain the event queue behind the first one's back.
    static TICKING: Cell<bool> = const { Cell::new(false) };
}

/// Records the player for later command access. Called by [`Player::new`], so
/// no caller has to remember to.
pub fn set_installed(p: Player) {
    INSTALLED.with(|i| *i.borrow_mut() = Some(p));
}

/// Runs `f` against the player. Returns `None` off the main thread or
/// before the player is created - never blocks, so a command that arrives
/// early fails fast instead of hanging.
pub fn with_player<R>(f: impl FnOnce(&Player) -> R) -> Option<R> {
    INSTALLED.with(|i| i.borrow().as_ref().map(f))
}

/// Installs a main-thread timer handing the player to `f` every `interval`.
///
/// The state feed needs a heartbeat: mpv reports first frame, end of file and
/// cache movement through an event queue that somebody has to drain, and an
/// undrained queue eventually overflows. Idempotent - a second call is a
/// no-op, because two tickers would each emit a state event per change.
///
/// Must be called on the main thread.
pub fn start_ticker(interval: std::time::Duration, mut f: impl FnMut(&Player) + 'static) {
    if TICKING.with(|t| t.replace(true)) {
        return;
    }
    crate::surface::repeat_on_main(interval, move || {
        with_player(&mut f);
        true
    });
}

/// What mpv reported since the last pump. Plain data: it crosses back to the
/// command thread, where the surface's types may not go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    StartFile,
    FileLoaded,
    /// First frame of a new size - the honest "playing" signal. `file-loaded`
    /// fires before a single frame is decoded, so a stream that connects and
    /// then stalls would otherwise look healthy.
    VideoReconfig,
    /// Playback of a file ended.
    EndFile {
        /// mpv's error code, when it called it a failure rather than an
        /// ending.
        error: Option<i32>,
        /// True when it ended only because something else was loaded over it.
        /// The obituary of the *previous* stream arrives after the next one
        /// has already opened, and acting on it would knock a healthy channel
        /// change back to stopped.
        replaced: bool,
    },
    /// mpv's `track-list`, as JSON, after it changed.
    Tracks(String),
    /// A warning or error mpv logged. Only ever read for the clues in
    /// `ipc::ending`, and never logged or shown: the text can carry the
    /// stream URL, and with it the account.
    Log(String),
}

/// One mpv event as a [`Signal`], or `None` for the ones nothing reads.
///
/// Separate from [`Player::pump`] so that a harness running its own mpv
/// reads events exactly as the app does.
pub fn signal_of(ev: libmpv2::Result<libmpv2::events::Event<'_>>) -> Option<Signal> {
    use libmpv2::events::Event;
    match ev {
        Ok(Event::StartFile) => Some(Signal::StartFile),
        Ok(Event::FileLoaded) => Some(Signal::FileLoaded),
        Ok(Event::VideoReconfig) => Some(Signal::VideoReconfig),
        Ok(Event::EndFile(reason)) => Some(Signal::EndFile {
            error: None,
            replaced: matches!(
                reason,
                libmpv2::mpv_end_file_reason::Stop
                    | libmpv2::mpv_end_file_reason::Redirect
                    | libmpv2::mpv_end_file_reason::Quit
            ),
        }),
        Ok(Event::PropertyChange {
            name: "track-list",
            change: libmpv2::events::PropertyData::Str(json),
            ..
        }) => Some(Signal::Tracks(json.to_owned())),
        Ok(Event::LogMessage { text, .. }) => Some(Signal::Log(text.to_owned())),
        Ok(Event::QueueOverflow) => {
            tracing::warn!("mpv event queue overflowed; some state changes were lost");
            None
        }
        Ok(_) => None,
        // libmpv2 folds a failed `end-file` into `Err`, so this is the only
        // place a playback failure is reported.
        Err(e) => Some(Signal::EndFile {
            error: Some(match e {
                libmpv2::Error::Raw(code) => code,
                // Not mpv's: bad UTF-8 in an event. Generic, rather than
                // pretending to know which failure it was.
                _ => -20,
            }),
            replaced: false,
        }),
    }
}

/// A one-shot reading for the diagnostics panel.
///
/// Every field is optional on purpose: mpv drops most of these between
/// files, and half of them never exist for a raw MPEG-TS multiplex. A
/// missing reading is a dash on screen, not a failure.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub container: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub hwdec: Option<String>,
    pub pixel_format: Option<String>,
    pub primaries: Option<String>,
    pub gamma: Option<String>,
    pub video_bitrate: Option<f64>,
    pub audio_bitrate: Option<f64>,
    pub measured_fps: Option<f64>,
    pub container_fps: Option<f64>,
    pub audio_samplerate: Option<i64>,
    pub audio_channels: Option<i64>,
    pub cache_duration: Option<f64>,
    pub cache_speed: Option<f64>,
    pub demuxer_bytes: Option<i64>,
    pub buffering: Option<bool>,
    pub cache_buffering_percent: Option<f64>,
    pub dropped_frames: Option<i64>,
    pub decoder_dropped_frames: Option<i64>,
    pub avsync: Option<f64>,
}

/// A snapshot of the properties the UI shows. Plain data, same reason.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Probe {
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
    pub codec: Option<String>,
    /// Seconds between the newest demuxed packet and where playback is.
    pub behind_live: f64,
    /// Seconds of scrollback the demuxer is still holding.
    pub buffered_back: f64,
    pub paused: bool,
    /// The end of mpv's seekable window, in stream time - the furthest
    /// forward a seek can actually land.
    pub live_edge: Option<f64>,
    pub time_pos: Option<f64>,
    /// The newest packet mpv holds, in stream time. Unlike `live_edge` it
    /// moves with every packet read, so it is what tells how fast data is
    /// arriving — and it moves while playback is paused.
    pub cache_end: Option<f64>,
    /// The demuxer has read the end of the stream: the server finished the
    /// response, or the connection failed. Playback carries on through
    /// what is buffered, so this comes well before `end-file`.
    pub demuxer_eof: bool,
    /// Bytes buffered ahead of the playhead.
    pub fw_bytes: Option<i64>,
    /// Bytes in the whole cache, behind the playhead and ahead. It grows
    /// with every packet that arrives until the back buffer is full, so its
    /// growth against `cache_end`'s is the stream's bytes per second.
    pub total_bytes: Option<i64>,
}

/// `demuxer-cache-state`, the parts of it that are read.
#[derive(Debug, Clone, Copy, Default)]
struct CacheWindow {
    /// The seekable window, which is not the cache: see
    /// [`Player::cache_window`].
    start: Option<f64>,
    end: Option<f64>,
    /// Where the demuxer is reading.
    reader: Option<f64>,
    cache_end: Option<f64>,
    eof: bool,
    fw_bytes: Option<i64>,
    total_bytes: Option<i64>,
}

/// Logged once if mpv will not hand back `demuxer-cache-state`, so the
/// degraded scrollback figure is explained exactly once rather than never or
/// four times a second.
static CACHE_STATE_WARNED: AtomicBool = AtomicBool::new(false);

impl Player {
    /// Sends a raw mpv command, as written in `input.conf`.
    pub fn command(&self, name: &str, args: &[&str]) -> Result<()> {
        Self::mpv()?
            .command(name, args)
            .with_context(|| format!("mpv command {name}"))
    }

    pub fn set_flag(&self, key: &str, value: bool) -> Result<()> {
        Self::mpv()?
            .set_property(key, value)
            .with_context(|| format!("setting {key}"))
    }

    pub fn set_f64(&self, key: &str, value: f64) -> Result<()> {
        Self::mpv()?
            .set_property(key, value)
            .with_context(|| format!("setting {key}"))
    }

    /// `None` rather than `Err` throughout the getters: almost every property
    /// here is legitimately unavailable before the first frame, and a caller
    /// polling for it does not want an error per tick.
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        Self::mpv().ok()?.get_property::<f64>(key).ok()
    }

    pub fn get_i64(&self, key: &str) -> Option<i64> {
        Self::mpv().ok()?.get_property::<i64>(key).ok()
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        Self::mpv().ok()?.get_property::<bool>(key).ok()
    }

    pub fn get_string(&self, key: &str) -> Option<String> {
        let s = Self::mpv().ok()?.get_property::<String>(key).ok()?;
        (!s.is_empty()).then_some(s)
    }

    /// Drains mpv's event queue.
    ///
    /// Somebody must: mpv buffers events and eventually reports
    /// `QueueOverflow`, and the "first frame arrived" edge exists nowhere
    /// else. Called from the ticker, on the main thread.
    pub fn pump(&self) -> Vec<Signal> {
        let Ok(mpv) = Self::mpv() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        // 0.0 polls rather than waits; this runs on the main thread and must
        // never block it.
        while let Some(ev) = mpv.wait_event(0.0) {
            out.extend(signal_of(ev));
        }
        out
    }

    /// The window mpv will actually seek within, plus where it is reading.
    ///
    /// Two traps live here, both found by measuring against a real server
    /// rather than read out of the manual.
    ///
    /// `demuxer-cache-state` is an `MPV_FORMAT_NODE` map whose handler
    /// implements only a plain GET, so the `demuxer-cache-state/cache-end`
    /// path does not resolve. The whole node is read as a string instead -
    /// mpv renders a node as JSON - and parsed here.
    ///
    /// And the seekable window is **not** `cache-begin`..`cache-end`. There
    /// is no `cache-begin` key at all, and `seekable-ranges` ends one to three
    /// seconds short of `cache-end`, because the tail of the cache is buffered
    /// but not yet seekable. A seek past that end is refused silently -
    /// playback simply stays where it was - so this is the number that must
    /// drive both the live edge and the forward clamp.
    fn cache_window(&self) -> Option<CacheWindow> {
        let raw = self.get_string("demuxer-cache-state")?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let ranges = v.get("seekable-ranges").and_then(|r| r.as_array());
        let bound = |which: fn(&[serde_json::Value]) -> Option<&serde_json::Value>, key| {
            ranges
                .and_then(|r| which(r))
                .and_then(|r| r.get(key))
                .and_then(|x| x.as_f64())
        };
        let cache_end = v.get("cache-end").and_then(|x| x.as_f64());
        Some(CacheWindow {
            start: bound(<[_]>::first, "start"),
            end: bound(<[_]>::last, "end").or(cache_end),
            reader: v.get("reader-pts").and_then(|x| x.as_f64()),
            cache_end,
            eof: v.get("eof").and_then(|x| x.as_bool()).unwrap_or(false),
            fw_bytes: v.get("fw-bytes").and_then(|x| x.as_i64()),
            total_bytes: v.get("total-bytes").and_then(|x| x.as_i64()),
        })
    }

    /// Everything the state feed needs, in one main-thread visit.
    /// Everything the diagnostics panel shows, read in one pass.
    ///
    /// Separate from [`Player::probe`] because it is far more than the state
    /// feed needs and is only read while the panel is open. Every field is
    /// optional: mpv drops most of these between files, and a missing
    /// reading is a dash in the interface rather than an error.
    ///
    /// **No stream URL, ever.** It carries the account's username and
    /// password in its query string, and this panel exists to be screenshotted
    /// and pasted into a bug report.
    pub fn stats(&self) -> Stats {
        Stats {
            // Container and codecs.
            container: self.get_string("file-format"),
            video_codec: self.get_string("video-codec"),
            audio_codec: self.get_string("audio-codec"),
            hwdec: self.get_string("hwdec-current"),
            pixel_format: self.get_string("video-params/pixelformat"),
            // Colour, which is what an HDR stream on an SDR screen shows up in.
            primaries: self.get_string("video-params/primaries"),
            gamma: self.get_string("video-params/gamma"),
            // Rates.
            video_bitrate: self.get_f64("video-bitrate"),
            audio_bitrate: self.get_f64("audio-bitrate"),
            measured_fps: self.get_f64("estimated-vf-fps"),
            container_fps: self.get_f64("container-fps"),
            // Audio format, the thing that made the compressor stall.
            audio_samplerate: self.get_i64("audio-params/samplerate"),
            audio_channels: self.get_i64("audio-params/channel-count"),
            // Health.
            cache_duration: self.get_f64("demuxer-cache-duration"),
            cache_speed: self.get_f64("cache-speed"),
            demuxer_bytes: self.get_i64("demuxer-cache-state/total-bytes"),
            buffering: self.get_bool("paused-for-cache"),
            cache_buffering_percent: self.get_f64("cache-buffering-state"),
            dropped_frames: self.get_i64("frame-drop-count"),
            decoder_dropped_frames: self.get_i64("decoder-frame-drop-count"),
            avsync: self.get_f64("avsync"),
        }
    }

    /// What the channel list remembers about a channel, read once a live
    /// stream has been playing for a few seconds (see `ipc::player_cmds`).
    ///
    /// Interlacing depends on the deinterlace mode, because
    /// `video-frame-info` describes the frame as shown, after any
    /// deinterlacer has made it progressive (measured: an interlaced clip
    /// reads `false` there under `auto`, `true` under `no`). Under `auto`
    /// the filter runs only for interlaced frames, so `deinterlace-active`
    /// is the answer; with it off, the frame is; forced on, the filter runs
    /// regardless and nothing can tell.
    pub fn reading(&self) -> crate::catalog::store::Reading {
        let interlaced = match self.get_string("deinterlace").as_deref() {
            Some("auto") => self.get_bool("deinterlace-active"),
            Some("yes") => None,
            _ => self.get_bool("video-frame-info/interlaced"),
        };
        let gamma = self.get_string("video-params/gamma");
        crate::catalog::store::Reading {
            codec: self.get_string("video-format"),
            width: self.get_i64("video-params/w"),
            height: self.get_i64("video-params/h"),
            fps: self
                .get_f64("container-fps")
                .or_else(|| self.get_f64("estimated-vf-fps"))
                .filter(|f| f.is_finite() && *f > 0.0),
            interlaced,
            // PQ is HDR10 and its kin, HLG broadcast HDR.
            is_hdr: matches!(gamma.as_deref(), Some("pq" | "hlg")),
            audio_codec: self.get_string("audio-codec-name"),
            audio_channels: self.get_i64("audio-params/channel-count"),
        }
    }

    /// Just the numbers the diagnostics graphs plot.
    ///
    /// `stats()` reads twenty-odd properties and most of them are static text
    /// about codecs and colour; this runs on every heartbeat, so it reads the
    /// four that move and nothing else.
    pub fn health(&self) -> crate::ipc::health::Reading {
        crate::ipc::health::Reading {
            cache_duration: self.get_f64("demuxer-cache-duration"),
            cache_speed: self.get_f64("cache-speed"),
            dropped_total: self.get_i64("frame-drop-count").unwrap_or(0)
                + self.get_i64("decoder-frame-drop-count").unwrap_or(0),
            buffering: self.get_bool("paused-for-cache").unwrap_or(false),
        }
    }

    pub fn probe(&self) -> Probe {
        let time_pos = self.get_f64("time-pos");
        let w = match self.cache_window() {
            Some(w) => w,
            None => {
                // Silent before a file is loaded: the property is legitimately
                // unavailable then, and warning about it would fire once, at
                // startup, about nothing.
                if time_pos.is_some() && !CACHE_STATE_WARNED.swap(true, Ordering::AcqRel) {
                    tracing::warn!(
                        "demuxer-cache-state unreadable; scrollback depth will be approximate"
                    );
                }
                CacheWindow {
                    end: self.get_f64("demuxer-cache-time"),
                    ..CacheWindow::default()
                }
            }
        };
        let (begin, end, reader) = (w.start, w.end, w.reader);
        let pos = reader.or(time_pos);
        let behind_live = match (end, pos) {
            (Some(e), Some(p)) => (e - p).max(0.0),
            _ => 0.0,
        };
        let buffered_back = match (begin, pos) {
            (Some(b), Some(p)) => (p - b).max(0.0),
            // No cache window: everything played so far is the best guess.
            _ => time_pos.unwrap_or(0.0).max(0.0),
        };
        Probe {
            width: self.get_i64("video-params/w"),
            height: self.get_i64("video-params/h"),
            // `container-fps` is absent on a raw MPEG-TS multiplex, where the
            // measured rate is the only figure the stream ever reports.
            fps: self
                .get_f64("container-fps")
                .or_else(|| self.get_f64("estimated-vf-fps"))
                .filter(|f| f.is_finite() && *f > 0.0),
            codec: self.get_string("video-format"),
            behind_live,
            buffered_back,
            paused: self.get_bool("pause").unwrap_or(false),
            live_edge: end,
            time_pos,
            cache_end: w.cache_end,
            demuxer_eof: w.eof,
            fw_bytes: w.fw_bytes,
            total_bytes: w.total_bytes,
        }
    }

    /// Every transport property as mpv itself renders it, errors included.
    ///
    /// The seek behaviour of a live MPEG-TS cache is not documented anywhere
    /// that matches what a given server actually does, so the verification
    /// harness prints this rather than guessing. Carries timings only - never
    /// a URL, which would carry the account password.
    pub fn diagnostics(&self) -> String {
        const KEYS: &[&str] = &[
            "time-pos",
            "duration",
            "seekable",
            "partially-seekable",
            "demuxer-cache-time",
            "demuxer-cache-duration",
            "cache-buffering-state",
            "video-params/w",
            "video-params/h",
            "container-fps",
            "estimated-vf-fps",
            "video-format",
            "demuxer-cache-state",
        ];
        let Ok(m) = Self::mpv() else {
            return "no mpv in this process".to_owned();
        };
        KEYS.iter()
            .map(|k| match m.get_property::<String>(k) {
                Ok(v) => format!("{k}={v}"),
                Err(e) => format!("{k}=<{e}>"),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_with_no_player_gets_none_rather_than_a_hang() {
        // Every command hops to the main thread; one that arrives before
        // `setup` has built the player must fail immediately, not block on a
        // reply that will never come.
        assert!(with_player(|_| ()).is_none());
    }
}
