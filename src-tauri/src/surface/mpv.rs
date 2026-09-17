//! libmpv handle plus its OpenGL render context.
//!
//! Everything here is main-thread-only: `RenderContext` is `!Send`, and the
//! context can only be created while a GL context is current, which is true
//! inside the GLArea's render signal and nowhere earlier.
//!
//! No `connect_render` handler lives here on purpose. GTK3's `render` signal
//! stops emitting once a handler returns `TRUE`, so a second handler would be
//! a silent black screen. [`draw`] is called by the single handler in
//! [`crate::surface::overlay`] instead.

use anyhow::{anyhow, Context, Result};
use gtk::prelude::*;
use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};
use libmpv2::Mpv;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::surface::gl;

/// Set by mpv's update callback, cleared as we render. Must be `Send`, which
/// is why it is an atomic static and not part of the thread-local state.
static FRAME_READY: AtomicBool = AtomicBool::new(true);

/// One-shot so the render target is reported once, not sixty times a second.
static FBO_LOGGED: AtomicBool = AtomicBool::new(false);

/// Hand mpv an upside-down target: GL's origin is bottom-left, video's is
/// top-left.
///
/// Measured rather than assumed, because the wrong value renders perfectly and
/// is simply upside down. Played a clip whose top half is white into the
/// GLArea and read the framebuffer back: with `true`, the screen-top strip
/// reads 255 and the screen-bottom 0; with `false` that inverts exactly.
const FLIP_Y: bool = true;

thread_local! {
    static MPV: RefCell<Option<&'static Mpv>> = const { RefCell::new(None) };
    static CTX: RefCell<Option<RenderContext<'static>>> = const { RefCell::new(None) };
    static AREA: RefCell<Option<gtk::GLArea>> = const { RefCell::new(None) };
    /// A URL asked for before the render context existed. See [`Player::load`].
    static PENDING: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// A handle to the process-wide mpv instance.
///
/// Zero-sized: the real state lives in thread-locals because `RenderContext`
/// is `!Send` and must never leave the GTK main thread.
pub struct Player;

impl Player {
    /// Creates the mpv handle. The render context is deferred to first draw,
    /// because `mpv_render_context_create` needs a *current* GL context and
    /// the GLArea is not realized during Tauri's `setup`.
    pub fn new(area: &gtk::GLArea) -> Result<Self> {
        force_c_numeric_locale();
        let mpv = Mpv::with_initializer(|init| {
            // Live rewind: ~512 MiB, roughly 3.7 minutes at 18 Mbps.
            init.set_property("cache", "yes".to_owned())?;
            init.set_property("demuxer-max-back-bytes", 512i64 * 1024 * 1024)?;
            init.set_property("demuxer-max-bytes", 256i64 * 1024 * 1024)?;
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
            // HDR on an SDR panel must be tone-mapped, not washed out.
            init.set_property("tone-mapping", "auto".to_owned())?;
            init.set_property("target-colorspace-hint", "yes".to_owned())?;
            init.set_property("volume-max", 200i64)?;
            // Some servers reject unknown user agents; VLC's is widely
            // accepted.
            init.set_property("user-agent", "VLC/3.0.20 LibVLC/3.0.20".to_owned())?;
            init.set_property("network-timeout", 8i64)?;
            init.set_property("vo", "libmpv".to_owned())?;
            Ok(())
        })
        .context("initialising mpv")?;

        // Leaked deliberately: RenderContext borrows Mpv for 'a, and the
        // player outlives everything anyway. Avoids a self-referential struct.
        let mpv: &'static Mpv = Box::leak(Box::new(mpv));
        MPV.with(|m| *m.borrow_mut() = Some(mpv));
        AREA.with(|a| *a.borrow_mut() = Some(area.clone()));
        // Register on the way out so command code can reach the player without
        // `lib.rs` having to remember a second call.
        set_installed(Player);
        Ok(Self)
    }

    fn mpv() -> Result<&'static Mpv> {
        MPV.with(|m| *m.borrow())
            .ok_or_else(|| anyhow!("mpv not initialised on this thread"))
    }

    /// Starts playback, deferring if the GL surface is not ready yet.
    ///
    /// mpv initialises its video output as soon as a file opens. If that
    /// happens before the render context exists, the VO fails outright -
    /// "Error opening/initializing the selected video_out (--vo) device" - and
    /// nothing ever decodes. A network stream hides this by taking a second to
    /// open, which is long enough for the first frame to have been drawn; a
    /// local file opens instantly and fails every time. So a load requested
    /// before the first draw is held until the context is up.
    pub fn load(&self, url: &str) -> Result<()> {
        let ready = CTX.with(|c| c.borrow().is_some());
        if !ready {
            PENDING.with(|p| *p.borrow_mut() = Some(url.to_owned()));
            AREA.with(|a| {
                if let Some(a) = a.borrow().as_ref() {
                    a.queue_render();
                }
            });
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

/// glibc's `LC_NUMERIC`. Stable at 1 on Linux since forever; hardcoded
/// because binding libc for one integer is not worth a dependency.
const LC_NUMERIC: i32 = 1;

/// libmpv refuses to initialise unless `LC_NUMERIC` is `C`.
///
/// GTK calls `setlocale(LC_ALL, "")` during init, which on a machine with a
/// non-C locale makes `mpv_create` fail with "Non-C locale detected" — an
/// error that surfaces only as a generic init failure. `setlocale` is already
/// in the process (libc is always loaded), so it is resolved from there rather
/// than by pulling in a libc crate.
fn force_c_numeric_locale() {
    use libloading::os::unix::Library;

    let this = Library::this();
    let f = unsafe {
        this.get::<unsafe extern "C" fn(i32, *const std::ffi::c_char) -> *mut std::ffi::c_char>(
            b"setlocale\0",
        )
    };
    match f {
        Ok(setlocale) => unsafe {
            setlocale(LC_NUMERIC, c"C".as_ptr());
        },
        Err(e) => tracing::warn!("could not resolve setlocale; mpv may refuse to start: {e}"),
    }
}

/// Draws mpv's current frame. Returns false if mpv is not ready, so the
/// caller can clear instead. Called only from the GLArea render signal, where
/// a GL context is guaranteed current.
pub(crate) fn draw(area: &gtk::GLArea) -> bool {
    let Some(mpv) = MPV.with(|m| *m.borrow()) else {
        return false;
    };

    // Capture the render target FIRST, before anything else touches GL.
    //
    // NOT framebuffer 0. GtkGLArea binds its own FBO before emitting `render`
    // (gtk_gl_area_attach_buffers) and composites only that; the default
    // framebuffer is never shown, so drawing into 0 yields a black window with
    // a healthy render count, frames "painted", and no GL error.
    //
    // And it must be read *here* rather than just before `render()`: creating
    // mpv's render context leaves framebuffer 0 bound, so a query made after
    // that reports 0 on the first frame. Measured: 1 at entry,
    // 0 immediately after `create_render_context`.
    let fbo = unsafe { gl::get_integer(gl::DRAW_FRAMEBUFFER_BINDING) };

    CTX.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            match mpv.create_render_context(vec![
                RenderParam::ApiType(RenderParamApiType::OpenGl),
                RenderParam::InitParams(OpenGLInitParams {
                    get_proc_address: |_: &(), name: &str| gl::get_proc_address(name),
                    ctx: (),
                }),
            ]) {
                Ok(mut ctx) => {
                    // Must only touch Send data: flag it and let the GTK main
                    // loop find the area, because GLArea is !Send.
                    ctx.set_update_callback(|| {
                        if !FRAME_READY.swap(true, Ordering::AcqRel) {
                            gtk::glib::idle_add_once(|| {
                                AREA.with(|a| {
                                    if let Some(a) = a.borrow().as_ref() {
                                        a.queue_render();
                                    }
                                });
                            });
                        }
                    });
                    *slot = Some(ctx);
                    // Now that a video output can succeed, start anything that
                    // was asked for while the surface was still coming up.
                    if let Some(url) = PENDING.with(|p| p.borrow_mut().take()) {
                        if let Err(e) = mpv.command("loadfile", &[&url, "replace"]) {
                            tracing::error!("deferred loadfile: {e}");
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("creating mpv render context: {e}");
                    return false;
                }
            }
        }

        FRAME_READY.store(false, Ordering::Release);
        let scale = area.scale_factor();
        // gtk-rs 0.18 is GTK3: width()/height() are GTK4 APIs.
        let w = (area.allocated_width() * scale).max(1);
        let h = (area.allocated_height() * scale).max(1);

        if !FBO_LOGGED.swap(true, Ordering::AcqRel) {
            tracing::info!("mpv render target: GTK-bound FBO {fbo} at {w}x{h} (scale {scale})");
        }

        let rendered = slot.as_mut().unwrap().render::<()>(fbo, w, h, FLIP_Y);

        // mpv renders through its own FBOs and does not put GTK's back when it
        // finishes. GTK expects to own that binding for the rest of the frame,
        // and anything the caller does afterwards - reading the surface back,
        // clearing it - would otherwise land on whichever framebuffer mpv left
        // behind. Restoring it is also what stops mpv seeing a stale
        // INVALID_FRAMEBUFFER_OPERATION from someone else's read on its next
        // frame.
        unsafe { gl::bind_framebuffer(gl::FRAMEBUFFER, fbo) };

        match rendered {
            Ok(()) => true,
            Err(e) => {
                tracing::error!("mpv render: {e}");
                false
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Command-facing API: registry, event pump, and property probe.
//
// Everything below is main-thread-only for the same reason as the rest of this
// module: `MPV` is a thread-local. Tauri commands reach it through
// `AppHandle::run_on_main_thread` and [`with_player`].
// ---------------------------------------------------------------------------

thread_local! {
    /// The installed player, reachable only from the GTK main thread.
    ///
    /// `Player` is zero-sized, but the registry still has to exist: the real
    /// state is in `MPV`/`CTX`/`AREA`, which are thread-locals of *this*
    /// thread, so "is there a player here" is a per-thread question.
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

/// Runs `f` against the player. Returns `None` off the GTK main thread or
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
/// Must be called on the GTK main thread.
pub fn start_ticker(interval: std::time::Duration, mut f: impl FnMut(&Player) + 'static) {
    if TICKING.with(|t| t.replace(true)) {
        return;
    }
    gtk::glib::timeout_add_local(interval, move || {
        with_player(&mut f);
        gtk::glib::ControlFlow::Continue
    });
}

/// What mpv reported since the last pump. Plain data: it crosses back to the
/// command thread, where GTK types may not go.
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
        /// Set when mpv called it a failure rather than an ending.
        error: Option<String>,
        /// True when it ended only because something else was loaded over it.
        /// The obituary of the *previous* stream arrives after the next one
        /// has already opened, and acting on it would knock a healthy channel
        /// change back to stopped.
        replaced: bool,
    },
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
        // 0.0 polls rather than waits; this runs on the GTK main thread and
        // must never block it.
        while let Some(ev) = mpv.wait_event(0.0) {
            match ev {
                Ok(libmpv2::events::Event::StartFile) => out.push(Signal::StartFile),
                Ok(libmpv2::events::Event::FileLoaded) => out.push(Signal::FileLoaded),
                Ok(libmpv2::events::Event::VideoReconfig) => out.push(Signal::VideoReconfig),
                Ok(libmpv2::events::Event::EndFile(reason)) => out.push(Signal::EndFile {
                    error: None,
                    replaced: matches!(
                        reason,
                        libmpv2::mpv_end_file_reason::Stop
                            | libmpv2::mpv_end_file_reason::Redirect
                            | libmpv2::mpv_end_file_reason::Quit
                    ),
                }),
                Ok(libmpv2::events::Event::QueueOverflow) => {
                    tracing::warn!("mpv event queue overflowed; some state changes were lost");
                }
                Ok(_) => {}
                // libmpv2 folds a failed `end-file` into `Err`, so this is the
                // only place a playback failure is reported.
                Err(e) => out.push(Signal::EndFile {
                    error: Some(e.to_string()),
                    replaced: false,
                }),
            }
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
    fn cache_window(&self) -> Option<(Option<f64>, Option<f64>, Option<f64>)> {
        let raw = self.get_string("demuxer-cache-state")?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let ranges = v.get("seekable-ranges").and_then(|r| r.as_array());
        let bound = |which: fn(&[serde_json::Value]) -> Option<&serde_json::Value>, key| {
            ranges
                .and_then(|r| which(r))
                .and_then(|r| r.get(key))
                .and_then(|x| x.as_f64())
        };
        let start = bound(<[_]>::first, "start");
        let end = bound(<[_]>::last, "end").or_else(|| v.get("cache-end").and_then(|x| x.as_f64()));
        let reader = v.get("reader-pts").and_then(|x| x.as_f64());
        Some((start, end, reader))
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
        let (begin, end, reader) = match self.cache_window() {
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
                (None, self.get_f64("demuxer-cache-time"), None)
            }
        };
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
            return "no mpv on this thread".to_owned();
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
        // Every command hops to the GTK main thread; one that arrives before
        // `setup` has built the player must fail immediately, not block on a
        // reply that will never come.
        assert!(with_player(|_| ()).is_none());
    }
}
