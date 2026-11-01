//! mpv's OpenGL render context, drawing into the GtkGLArea.
//!
//! Everything here is main-thread-only: `RenderContext` is `!Send`, and the
//! context can only be created while a GL context is current, which is true
//! inside the GLArea's render signal and nowhere earlier.
//!
//! No `connect_render` handler lives here on purpose. GTK3's `render` signal
//! stops emitting once a handler returns `TRUE`, so a second handler would be
//! a silent black screen. [`draw`] is called by the single handler in
//! [`super::overlay`] instead.

use gtk::prelude::*;
use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use super::gl;
use crate::surface::mpv;

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
    static CTX: RefCell<Option<RenderContext<'static>>> = const { RefCell::new(None) };
    static AREA: RefCell<Option<gtk::GLArea>> = const { RefCell::new(None) };
}

/// Creates the player and binds it to `area`. The render context itself is
/// deferred to the first draw, because `mpv_render_context_create` needs a
/// *current* GL context and the GLArea is not realized during Tauri's
/// `setup`.
pub fn new_player(area: &gtk::GLArea) -> anyhow::Result<mpv::Player> {
    force_c_numeric_locale();
    let player = mpv::Player::new(|init| {
        init.set_property("vo", "libmpv".to_owned())?;
        // Don't let mpv wait out each frame's display time inside render().
        //
        // By default `mpv_render_context_render` blocks until the frame is
        // due, and here it runs on GTK's main thread — the one that also
        // composites WebKit's scrolling and takes input. Measured with a
        // 50fps stream on a 60Hz display: render() held that thread for
        // 17ms of every 16.7ms frame, so lists scrolled in stutters
        // whenever video played. With the offset at 0 it returns in 0.5ms,
        // and mpv's own counters showed no more dropped or late frames than
        // before (50fps and 60fps fixtures, 30s each): GTK's frame clock
        // already paces the redraws.
        init.set_property("video-timing-offset", 0i64)
    })?;
    AREA.with(|a| *a.borrow_mut() = Some(area.clone()));
    Ok(player)
}

/// Whether mpv can open a file yet: only once the render context exists.
/// See [`mpv::Player::load`].
pub(super) fn ready() -> bool {
    CTX.with(|c| c.borrow().is_some())
}

/// Asks GTK for a frame, which is what creates the render context.
pub(super) fn kick() {
    AREA.with(|a| {
        if let Some(a) = a.borrow().as_ref() {
            a.queue_render();
        }
    });
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

unsafe extern "C" {
    fn gdk_wayland_display_get_wl_display(
        display: *mut gtk::gdk::ffi::GdkDisplay,
    ) -> *mut std::ffi::c_void;
    fn gdk_x11_display_get_xdisplay(display: *mut gtk::gdk::ffi::GdkDisplay)
        -> *mut std::ffi::c_void;
}

/// The window system's display handle, for mpv's hardware-decoding interop.
///
/// Without it mpv cannot hand VA-API frames to OpenGL directly, and falls
/// back to a copy-back decoder: measured here as `vulkan-copy`, which decodes
/// on the GPU and then copies every frame through system memory to upload it
/// again. With the display, mpv picks `vaapi` and frames stay on the GPU:
/// measured on the 1080p motion fixture, 14% of a core with copy-back
/// against 10% without, same build — CPU that playback no longer has to win
/// from whatever else is running.
///
/// Both functions are part of libgdk-3, which is already linked; which one
/// applies is decided by the display's actual type, since a GTK3 build can
/// run on either backend.
fn native_display(area: &gtk::GLArea) -> Option<RenderParam<()>> {
    use gtk::glib::translate::ToGlibPtr;
    let display = WidgetExt::display(area);
    let raw: *mut gtk::gdk::ffi::GdkDisplay = display.to_glib_none().0;
    match display.type_().name() {
        "GdkWaylandDisplay" => {
            let wl = unsafe { gdk_wayland_display_get_wl_display(raw) };
            (!wl.is_null()).then_some(RenderParam::WaylandDisplay(wl as *const _))
        }
        "GdkX11Display" => {
            let x = unsafe { gdk_x11_display_get_xdisplay(raw) };
            (!x.is_null()).then_some(RenderParam::X11Display(x as *const _))
        }
        other => {
            tracing::warn!("no hardware-decoding interop for a {other}");
            None
        }
    }
}

/// Draws mpv's current frame. Returns false if mpv is not ready, so the
/// caller can clear instead. Called only from the GLArea render signal, where
/// a GL context is guaranteed current.
pub(crate) fn draw(area: &gtk::GLArea) -> bool {
    let Some(mpv) = mpv::handle() else {
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
            let mut params = vec![
                RenderParam::ApiType(RenderParamApiType::OpenGl),
                RenderParam::InitParams(OpenGLInitParams {
                    get_proc_address: |_: &(), name: &str| gl::get_proc_address(name),
                    ctx: (),
                }),
            ];
            params.extend(native_display(area));
            match mpv.create_render_context(params) {
                Ok(mut ctx) => {
                    // Must only touch Send data: flag it and let the GTK main
                    // loop find the area, because GLArea is !Send.
                    ctx.set_update_callback(|| {
                        if !FRAME_READY.swap(true, Ordering::AcqRel) {
                            gtk::glib::idle_add_once(kick);
                        }
                    });
                    *slot = Some(ctx);
                    // Now that a video output can succeed, start anything that
                    // was asked for while the surface was still coming up.
                    mpv::load_pending();
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
