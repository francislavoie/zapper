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
use std::cell::RefCell;
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
