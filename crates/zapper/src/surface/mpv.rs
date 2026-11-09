//! libmpv: the one player, its tuned options, its event queue, and the
//! properties the interface reads.
//!
//! Portable. How the picture reaches the screen is the platform's business
//! and enters here in exactly two places: the `video_out` options handed to
//! [`Player::new`] (Linux asks for `vo=libmpv` and renders through GL itself;
//! Windows gives mpv a window to draw into), and the readiness gate in
//! [`Player::load`].
//!
//! [`Player`] is the app's [`PlayerBackend`]: the platform's surface creates
//! it and installs it (`crate::player::install`), and the controller calls it
//! from whichever thread it is on. libmpv's client API is thread-safe; only
//! the render context, which draws, belongs to one thread, and that lives in
//! the platform's module, not here.

use anyhow::{anyhow, Context, Result};
use libmpv2::Mpv;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::player::{BackendCaps, PlayerBackend};
pub use crate::player::{Health, Probe, Signal, Stats};

/// The process's mpv. Leaked deliberately: Linux's render context borrows it
/// for `'static`, and the player outlives everything anyway, so a
/// self-referential struct is avoided. `Mpv` is `Send + Sync` (libmpv's
/// client API is thread-safe), which is what lets it be a plain static.
static MPV: OnceLock<&'static Mpv> = OnceLock::new();

/// A URL asked for before the surface could show it. See [`Player::load`].
///
/// A lock rather than a thread's local: a load is asked for on a command's
/// thread and let go on the surface's. Every `loadfile` is sent under it,
/// with the readiness check before it, so a surface that becomes ready
/// meanwhile cannot miss the URL (it sets its readiness before
/// [`load_pending`] takes this), and a held URL cannot be sent after a newer
/// one.
static PENDING: Mutex<Option<String>> = Mutex::new(None);

/// One pump at a time: `mpv_wait_event` must not run on two threads at once
/// for the same handle.
static PUMPING: Mutex<()> = Mutex::new(());

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
/// platform's surface keeps on its own thread. `Send + Sync`, so the surface
/// installs it as the process's [`PlayerBackend`].
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
        // The subtitle on screen with its ASS tags: where a closed caption
        // sits on its grid (`ipc::captions`). Not fatal: captions are then
        // drawn at the bottom, centred.
        if let Err(e) = mpv.observe_property("sub-text/ass", libmpv2::Format::String, 1) {
            tracing::warn!("could not observe the subtitle text: {e}");
        }

        let mpv: &'static Mpv = Box::leak(Box::new(mpv));
        MPV.set(mpv)
            .map_err(|_| anyhow!("mpv is already initialised in this process"))?;
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
    fn load_now_or_later(url: &str) -> Result<()> {
        let mut pending = PENDING.lock().unwrap_or_else(PoisonError::into_inner);
        if !crate::surface::platform::video_ready() {
            *pending = Some(url.to_owned());
            drop(pending);
            crate::surface::platform::request_frame();
            return Ok(());
        }
        // This one is newer than anything still held.
        *pending = None;
        Self::mpv()?
            .command("loadfile", &[url, "replace"])
            .context("loadfile")
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

/// The player [`Player::new`] made, for the harnesses, which read what the
/// controller does not ([`Player::dimensions`], [`Player::diagnostics`]).
/// `None` before it.
pub fn created() -> Option<Player> {
    handle().map(|_| Player)
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
/// a thread that draws or answers the interface; playback carries on
/// meanwhile.
pub fn dump_cache(mpv: &Mpv, start: f64, end: f64, path: &Path) -> Result<()> {
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
/// it. Called by the platform once its video output can succeed, after it has
/// made its `video_ready` true.
pub fn load_pending() {
    let mut pending = PENDING.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(url) = pending.take() else {
        return;
    };
    let Some(mpv) = handle() else { return };
    if let Err(e) = mpv.command("loadfile", &[&url, "replace"]) {
        tracing::error!("deferred loadfile: {e}");
    }
}

// ---------------------------------------------------------------------------
// The controller's side: the event pump, the property probe, and the rest of
// [`PlayerBackend`].
//
// Callable from any thread: libmpv's client API is thread-safe, and the
// handle is a process-wide static. What stays on the platform's own thread is
// the drawing (the render context, in `linux::render` and `macos`), which
// never comes through here.
// ---------------------------------------------------------------------------

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
        Ok(Event::PropertyChange {
            name: "sub-text/ass",
            change: libmpv2::events::PropertyData::Str(text),
            ..
        }) => Some(Signal::SubText(text.to_owned())),
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

impl PlayerBackend for Player {
    /// mpv does everything zapper asks of a player.
    fn capabilities(&self) -> BackendCaps {
        BackendCaps {
            rewind_buffer: true,
            cache_dump: true,
            audio_filters: true,
            caption_geometry: true,
        }
    }

    /// Starts playback, deferring if the surface is not ready yet: see
    /// [`Player::load_now_or_later`].
    fn load(&self, url: &str) -> Result<()> {
        Self::load_now_or_later(url)
    }

    fn set_property(&self, key: &str, value: &str) -> Result<()> {
        Self::mpv()?
            .set_property(key, value.to_owned())
            .with_context(|| format!("setting {key}"))
    }

    /// Sends a raw mpv command, as written in `input.conf`.
    fn command(&self, name: &str, args: &[&str]) -> Result<()> {
        Self::mpv()?
            .command(name, args)
            .with_context(|| format!("mpv command {name}"))
    }

    fn set_flag(&self, key: &str, value: bool) -> Result<()> {
        Self::mpv()?
            .set_property(key, value)
            .with_context(|| format!("setting {key}"))
    }

    fn set_f64(&self, key: &str, value: f64) -> Result<()> {
        Self::mpv()?
            .set_property(key, value)
            .with_context(|| format!("setting {key}"))
    }

    /// `None` rather than `Err` throughout the getters: almost every property
    /// here is legitimately unavailable before the first frame, and a caller
    /// polling for it does not want an error per tick.
    fn get_f64(&self, key: &str) -> Option<f64> {
        Self::mpv().ok()?.get_property::<f64>(key).ok()
    }

    fn get_i64(&self, key: &str) -> Option<i64> {
        Self::mpv().ok()?.get_property::<i64>(key).ok()
    }

    fn get_bool(&self, key: &str) -> Option<bool> {
        Self::mpv().ok()?.get_property::<bool>(key).ok()
    }

    fn get_string(&self, key: &str) -> Option<String> {
        let s = Self::mpv().ok()?.get_property::<String>(key).ok()?;
        (!s.is_empty()).then_some(s)
    }

    /// Drains mpv's event queue.
    ///
    /// Somebody must: mpv buffers events and eventually reports
    /// `QueueOverflow`, and the "first frame arrived" edge exists nowhere
    /// else. Called by the heartbeat, on its own thread.
    fn pump(&self) -> Vec<Signal> {
        let Ok(mpv) = Self::mpv() else {
            return Vec::new();
        };
        let _one_at_a_time = PUMPING.lock().unwrap_or_else(PoisonError::into_inner);
        let mut out = Vec::new();
        // 0.0 polls rather than waits: a pump never holds up its caller.
        while let Some(ev) = mpv.wait_event(0.0) {
            out.extend(signal_of(ev));
        }
        out
    }


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
    fn stats(&self) -> Stats {
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
    fn reading(&self) -> crate::catalog::store::Reading {
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
    fn health(&self) -> Health {
        Health {
            cache_duration: self.get_f64("demuxer-cache-duration"),
            cache_speed: self.get_f64("cache-speed"),
            dropped_total: self.get_i64("frame-drop-count").unwrap_or(0)
                + self.get_i64("decoder-frame-drop-count").unwrap_or(0),
            buffering: self.get_bool("paused-for-cache").unwrap_or(false),
        }
    }

    fn probe(&self) -> Probe {
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
            cache_start: w.start,
            demuxer_eof: w.eof,
            fw_bytes: w.fw_bytes,
            cache_bytes: w.total_bytes,
            duration: self
                .get_f64("duration")
                .filter(|d| d.is_finite() && *d > 0.0),
        }
    }

    fn dump_cache(&self, start: f64, end: f64, path: &Path) -> Result<()> {
        dump_cache(Self::mpv()?, start, end, path)
    }
}

impl Player {
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

