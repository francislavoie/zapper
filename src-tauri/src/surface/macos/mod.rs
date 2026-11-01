//! The macOS surface: AppKit, Core Animation and OpenGL.
//!
//! mpv on macOS has no way to draw into a view it is handed (`wid` is not
//! supported there), so this is Linux's arrangement in AppKit's terms:
//!
//! - A view of ours, [`VideoHost`], is put into the window beneath the
//!   WKWebView, as its sibling, filling the same area. It holds one child,
//!   the video view, whose frame is the rect the page asks for.
//! - The video view hosts [`VideoLayer`], a `CAOpenGLLayer`. Core Animation
//!   makes its GL context current and calls the layer to draw whenever the
//!   layer is marked as needing display; mpv renders into it there, through
//!   the OpenGL render API (`vo=libmpv`), and mpv's update callback is what
//!   marks it.
//! - The webview is transparent (wry's `drawsBackground`, through Tauri's
//!   background colour), so wherever the page has no background of its own
//!   the video shows through, as on Linux and Windows. The window under it is
//!   black, which is what shows where the video does not reach.
//!
//! The layer is not asynchronous: it draws only when mpv says there is a new
//! frame (or the view changed size), on the main thread, during Core
//! Animation's commit. That is the thread mpv's render context is created on
//! and the only one it is used from.
//!
//! Neither view takes input: the host answers every hit test with nothing,
//! so clicks reach the webview above even where the page is transparent.
//!
//! Scrollback thumbnails come from mpv's `screenshot-raw`
//! ([`crate::surface::screenshot`]): the layer's framebuffer is current only
//! inside Core Animation's draw call, and a thumbnail does not need to wait
//! for one.

#![allow(deprecated)] // OpenGL and CAOpenGLLayer are deprecated, not removed.

use std::cell::{Cell, RefCell};
use std::ffi::{c_void, CString};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, NSObject, NSObjectProtocol};
use objc2::{define_class, msg_send, AllocAnyThread, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSResponder, NSView, NSWindow, NSWindowOrderingMode,
};
use objc2_core_video::CVTimeStamp;
use objc2_foundation::{NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSTimer};
use objc2_open_gl::{
    CGLChoosePixelFormat, CGLContextObj, CGLError, CGLOpenGLProfile, CGLPixelFormatAttribute,
    CGLPixelFormatObj,
};
use objc2_quartz_core::{CALayer, CAOpenGLLayer, CATransaction};

use crate::surface::{mpv, screenshot, VideoRect};

/// Hand mpv an upside-down target: GL's origin is bottom-left, video's is
/// top-left, and the layer's framebuffer is an ordinary GL one.
const FLIP_Y: bool = true;

const GL_COLOR_BUFFER_BIT: u32 = 0x0000_4000;
const GL_FRAMEBUFFER: u32 = 0x8D40;
/// `GL_DRAW_FRAMEBUFFER_BINDING`: Core Animation binds a framebuffer of its
/// own before calling the layer, which may or may not be 0.
const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
const GL_VIEWPORT: u32 = 0x0BA2;
const GL_RGBA: u32 = 0x1908;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_RENDERER: u32 = 0x1F01;
const GL_VERSION: u32 = 0x1F02;

thread_local! {
    /// The installed views and layer, on the main thread.
    static SURFACE: RefCell<Option<Surface>> = const { RefCell::new(None) };
    /// Where the video goes, normalised. Kept so a resize can reapply it.
    static PLACED: Cell<VideoRect> = Cell::new(VideoRect::full());
    /// mpv's render context, created inside the layer's first draw: it needs
    /// the layer's GL context current, and that is the only place it is.
    static CTX: RefCell<Option<RenderContext<'static>>> = const { RefCell::new(None) };
}

struct Surface {
    window: Retained<NSWindow>,
    host: Retained<VideoHost>,
    view: Retained<NSView>,
    layer: Retained<VideoLayer>,
}

/// Set by mpv's update callback, cleared as the layer draws, so a burst of
/// updates between two draws asks the main thread for one redraw.
static FRAME_READY: AtomicBool = AtomicBool::new(true);

/// What the verification harness asserts on: how often the layer drew, how
/// many of those mpv painted, and the brightest value seen in the centre of
/// the surface while sampling is on. A layer that never draws, or draws
/// black, looks healthy from everywhere else.
static DRAWS: AtomicU32 = AtomicU32::new(0);
static MPV_DRAWS: AtomicU32 = AtomicU32::new(0);
static PEAK: AtomicU8 = AtomicU8::new(0);
static SAMPLING: AtomicBool = AtomicBool::new(false);
static TARGET_LOGGED: AtomicBool = AtomicBool::new(false);

pub(super) fn install(window: &tauri::WebviewWindow) -> anyhow::Result<()> {
    use anyhow::{anyhow, Context};

    let mtm = MainThreadMarker::new().ok_or_else(|| anyhow!("not on the main thread"))?;
    if gl().is_none() {
        return Err(anyhow!(
            "OpenGL.framework did not load; there is nothing to draw video with"
        ));
    }

    // The page must not paint an opaque white sheet over the video, and the
    // window under it is black, as on Windows.
    let webview: &tauri::Webview = window.as_ref();
    webview
        .set_background_color(Some(tauri::window::Color(0, 0, 0, 0)))
        .context("making the webview transparent")?;
    webview
        .window()
        .set_background_color(Some(tauri::window::Color(0, 0, 0, 255)))
        .context("painting the window black")?;

    let ns_window = window
        .ns_window()
        .context("the main window has no NSWindow")?;
    let ns_window: Retained<NSWindow> =
        unsafe { Retained::retain(ns_window.cast::<NSWindow>()) }
            .ok_or_else(|| anyhow!("the main window's NSWindow is null"))?;
    let content = ns_window
        .contentView()
        .ok_or_else(|| anyhow!("the main window has no content view"))?;
    let web = find_webview(&content).ok_or_else(|| anyhow!("no WKWebView in the main window"))?;
    let parent =
        unsafe { web.superview() }.ok_or_else(|| anyhow!("the WKWebView has no superview"))?;

    let host = VideoHost::new(mtm, parent.bounds());
    host.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    host.setAutoresizesSubviews(true);
    // Beneath the webview, so the page is above the video and every click
    // lands on the page.
    let below: &NSView = &web;
    parent.addSubview_positioned_relativeTo(&host, NSWindowOrderingMode::Below, Some(below));

    let layer = VideoLayer::new();
    layer.setAsynchronous(false);
    layer.setNeedsDisplayOnBoundsChange(true);
    layer.setOpaque(true);
    let view = NSView::initWithFrame(NSView::alloc(mtm), host.bounds());
    // A layer-hosting view: the layer is set before `wantsLayer`, so AppKit
    // uses ours rather than making one.
    let hosted: &CALayer = &layer;
    view.setLayer(Some(hosted));
    view.setWantsLayer(true);
    host.addSubview(&view);

    SURFACE.with(|s| {
        *s.borrow_mut() = Some(Surface {
            window: ns_window,
            host,
            view,
            layer,
        })
    });
    place();

    force_c_numeric_locale();
    mpv::Player::new(|init| {
        init.set_property("vo", "libmpv".to_owned())?;
        // Render without waiting out each frame's display time: the draw runs
        // on the main thread, which also runs the interface, and Core
        // Animation already paces the redraws (the same reasoning as Linux's,
        // `linux::render`).
        init.set_property("video-timing-offset", 0i64)
    })?;

    repeat_on_main(screenshot::THUMB_POLL, || {
        screenshot::thumbnail_tick();
        true
    });
    kick();
    tracing::info!("video surface: OpenGL layer beneath the webview");
    Ok(())
}

/// The WKWebView in `view`'s subtree: wry puts it in a container of its own
/// as the window's content view, or straight into tao's.
fn find_webview(view: &NSView) -> Option<Retained<NSView>> {
    let class = AnyClass::get(c"WKWebView")?;
    let mut stack = vec![view.retain()];
    while let Some(v) = stack.pop() {
        if v.isKindOfClass(class) {
            return Some(v);
        }
        stack.extend(v.subviews().iter());
    }
    None
}

/// Puts the video view where [`PLACED`] says, in the host's current size.
fn place() {
    SURFACE.with(|s| {
        let s = s.borrow();
        let Some(s) = s.as_ref() else { return };
        let bounds = s.host.bounds();
        let scale = s.window.backingScaleFactor().max(1.0);
        let (w, h) = (bounds.size.width * scale, bounds.size.height * scale);
        if w < 1.0 || h < 1.0 {
            // Minimised. Keep the last placement rather than collapse to a pixel.
            return;
        }
        // In device pixels, then back to points, so the edges land on pixels.
        let (x, y, vw, vh) = PLACED.with(Cell::get).to_pixels(w as i32, h as i32);
        let frame = NSRect::new(
            NSPoint::new(f64::from(x) / scale, f64::from(y) / scale),
            NSSize::new(f64::from(vw) / scale, f64::from(vh) / scale),
        );
        // A layer's geometry animates by default; video following the page's
        // own animation frame by frame must not trail behind it.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        s.view.setFrame(frame);
        s.layer.setContentsScale(scale);
        s.layer.setNeedsDisplay();
        CATransaction::commit();
    });
}

pub(super) fn set_video_rect(rect: VideoRect) -> Result<(), String> {
    if SURFACE.with(|s| s.borrow().is_none()) {
        return Err("the video surface is not installed".to_owned());
    }
    PLACED.with(|p| p.set(rect));
    place();
    Ok(())
}

/// An `NSTimer` on the main run loop, in the common modes so it keeps firing
/// while a window is being resized or a menu is open.
pub(super) fn repeat_on_main(interval: Duration, f: impl FnMut() -> bool + 'static) {
    let f = RefCell::new(f);
    let block = block2::RcBlock::new(move |timer: NonNull<NSTimer>| {
        // A callback that somehow runs the run loop and re-enters itself is
        // skipped rather than allowed to panic on the borrow.
        let keep = match f.try_borrow_mut() {
            Ok(mut f) => f(),
            Err(_) => true,
        };
        if !keep {
            unsafe { timer.as_ref() }.invalidate();
        }
    });
    let timer = unsafe {
        NSTimer::timerWithTimeInterval_repeats_block(interval.as_secs_f64(), true, &block)
    };
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
}

/// Whether mpv can open a file yet: only once the render context exists.
/// See [`mpv::Player::load`].
pub(super) fn video_ready() -> bool {
    CTX.with(|c| c.borrow().is_some())
}

/// Asks Core Animation for a draw, which is also what creates the render
/// context.
pub(super) fn request_frame() {
    kick();
}

fn kick() {
    SURFACE.with(|s| {
        if let Some(s) = s.borrow().as_ref() {
            s.layer.setNeedsDisplay();
        }
    });
}

/// The development capture reconstructs GTK's two layers and has no macOS
/// counterpart; `screencapture` takes the real window instead.
pub(super) fn request_capture(_path: PathBuf, _after: Duration) {
    eprintln!("CAPTURE FAILED: ZAPPER_CAPTURE is only implemented on Linux");
}

/// How many times the layer has drawn, how many of those mpv painted, and
/// the brightest channel seen in the centre of the surface since sampling
/// began (see [`sample_centre`]).
pub fn stats() -> (u32, u32, u8) {
    (
        DRAWS.load(Ordering::Acquire),
        MPV_DRAWS.load(Ordering::Acquire),
        PEAK.load(Ordering::Acquire),
    )
}

/// The video view's frame and the size of the area it is placed in, in
/// points, top-left origin. `None` before [`install`] or off the main thread.
pub fn geometry() -> Option<([f64; 4], [f64; 2])> {
    SURFACE.with(|s| {
        s.borrow().as_ref().map(|s| {
            let f = s.view.frame();
            let b = s.host.bounds().size;
            (
                [f.origin.x, f.origin.y, f.size.width, f.size.height],
                [b.width, b.height],
            )
        })
    })
}

/// Reads back the centre of every frame drawn from now on, for the
/// harness's "is it actually showing something" check. Costs a GPU round
/// trip per frame, so it is never on in the app.
pub fn sample_centre(on: bool) {
    SAMPLING.store(on, Ordering::Release);
}

// ---------------------------------------------------------------------------
// The views
// ---------------------------------------------------------------------------

define_class!(
    /// Fills the webview's container beneath it and holds the video view.
    ///
    /// SAFETY: NSView has no subclassing requirements beyond what is
    /// overridden here, and none of these methods call into anything that
    /// could re-enter AppKit's layout.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZapperVideoHost"]
    struct VideoHost;

    impl VideoHost {
        /// Top-left origin, as the page's rect has.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        /// Never the target of a click, so where the page is transparent the
        /// click still reaches the webview.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }

        /// The window changed size: the video keeps its share of it.
        #[unsafe(method(resizeSubviewsWithOldSize:))]
        fn resize_subviews(&self, _old: NSSize) {
            place();
        }
    }
);

impl VideoHost {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

define_class!(
    /// The layer mpv draws into.
    ///
    /// SAFETY: CAOpenGLLayer's documented subclassing points are overridden,
    /// and the draw calls through to its own implementation, which flushes.
    #[unsafe(super(CAOpenGLLayer, CALayer, NSObject))]
    #[name = "ZapperVideoLayer"]
    struct VideoLayer;

    impl VideoLayer {
        /// A 3.2 core profile on an accelerated renderer: CAOpenGLLayer's own
        /// choice is the legacy 2.1 profile, which mpv's renderer can use only
        /// in part.
        #[unsafe(method(copyCGLPixelFormatForDisplayMask:))]
        fn copy_pixel_format(&self, mask: u32) -> CGLPixelFormatObj {
            if let Some(pf) = core_profile(mask) {
                return pf;
            }
            tracing::warn!("no OpenGL 3.2 core pixel format; falling back to Core Animation's");
            let pf: CGLPixelFormatObj =
                unsafe { msg_send![super(self), copyCGLPixelFormatForDisplayMask: mask] };
            if pf.is_null() {
                tracing::error!("no OpenGL pixel format at all; the video layer cannot draw");
            }
            pf
        }

        #[unsafe(method(drawInCGLContext:pixelFormat:forLayerTime:displayTime:))]
        fn draw_in_context(
            &self,
            ctx: CGLContextObj,
            pf: CGLPixelFormatObj,
            t: f64,
            ts: *const CVTimeStamp,
        ) {
            draw();
            unsafe {
                let _: () = msg_send![
                    super(self),
                    drawInCGLContext: ctx,
                    pixelFormat: pf,
                    forLayerTime: t,
                    displayTime: ts
                ];
            }
        }
    }
);

impl VideoLayer {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// An OpenGL 3.2 core pixel format for the displays in `mask`: a hardware
/// renderer if there is one, otherwise any renderer that has the profile.
///
/// The second is not a theoretical case. A Mac with no GPU of its own to
/// offer (a virtual machine, a CI runner) has only Apple's software
/// renderer, which drives no display in particular and is not
/// "accelerated", so asking for either finds nothing.
fn core_profile(mask: u32) -> Option<CGLPixelFormatObj> {
    use CGLPixelFormatAttribute as A;
    let core = [
        A::CGLPFAOpenGLProfile.0,
        CGLOpenGLProfile::CGLOGLPVersion_3_2_Core.0,
    ];
    let hardware = [
        A::CGLPFAAccelerated.0,
        A::CGLPFADoubleBuffer.0,
        A::CGLPFAAllowOfflineRenderers.0,
        A::CGLPFADisplayMask.0,
        mask,
    ];
    let any = [A::CGLPFADoubleBuffer.0, A::CGLPFAAllowOfflineRenderers.0];
    for (what, rest) in [("hardware", &hardware[..]), ("any renderer", &any[..])] {
        let attrs: Vec<u32> = core.iter().chain(rest).copied().chain([0]).collect();
        let mut pf: CGLPixelFormatObj = std::ptr::null_mut();
        let mut n: i32 = 0;
        let err = unsafe {
            CGLChoosePixelFormat(
                NonNull::new(attrs.as_ptr() as *mut CGLPixelFormatAttribute)?,
                NonNull::from(&mut pf),
                NonNull::from(&mut n),
            )
        };
        if err == CGLError::NoError && !pf.is_null() {
            tracing::info!("OpenGL 3.2 core pixel format: {what} ({n} screens)");
            return Some(pf);
        }
        tracing::info!(
            "no OpenGL 3.2 core pixel format with {what}: CGL error {}",
            err.0
        );
    }
    None
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// Draws mpv's current frame into the layer's framebuffer. Called by Core
/// Animation with the layer's GL context current.
fn draw() {
    DRAWS.fetch_add(1, Ordering::AcqRel);
    FRAME_READY.store(false, Ordering::Release);
    if MainThreadMarker::new().is_none() {
        // The render context is a main-thread local; a draw anywhere else
        // would make a second one against the same handle.
        tracing::error!("the video layer drew off the main thread; skipping");
        return;
    }
    // Read first, before mpv's render context (or its creation) binds
    // anything of its own.
    let fbo = unsafe { get_integer(GL_DRAW_FRAMEBUFFER_BINDING) };
    let mut viewport = [0i32; 4];
    unsafe { get_integers(GL_VIEWPORT, &mut viewport) };
    let (w, h) = (viewport[2].max(1), viewport[3].max(1));

    let painted = CTX.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let Some(mpv) = mpv::handle() else {
                return false;
            };
            let params = vec![
                RenderParam::ApiType(RenderParamApiType::OpenGl),
                RenderParam::InitParams(OpenGLInitParams {
                    get_proc_address: |_: &(), name: &str| get_proc_address(name),
                    ctx: (),
                }),
            ];
            match mpv.create_render_context(params) {
                Ok(mut ctx) => {
                    // From mpv's thread: only flag it and let the main queue
                    // ask for the draw.
                    ctx.set_update_callback(|| {
                        if !FRAME_READY.swap(true, Ordering::AcqRel) {
                            dispatch2::DispatchQueue::main().exec_async(kick);
                        }
                    });
                    *slot = Some(ctx);
                    // Start anything that was asked for while the surface was
                    // still coming up.
                    mpv::load_pending();
                }
                Err(e) => {
                    tracing::error!("creating mpv render context: {e}");
                    return false;
                }
            }
        }
        if !TARGET_LOGGED.swap(true, Ordering::AcqRel) {
            let renderer = unsafe { gl_string(GL_RENDERER) };
            let version = unsafe { gl_string(GL_VERSION) };
            tracing::info!(
                "mpv render target: Core Animation FBO {fbo} at {w}x{h}, {renderer}, OpenGL {version}"
            );
        }
        let rendered = slot.as_ref().unwrap().render::<()>(fbo, w, h, FLIP_Y);
        // mpv leaves its own framebuffer bound; Core Animation's must be the
        // one bound when the layer hands back.
        unsafe { bind_framebuffer(fbo) };
        match rendered {
            Ok(()) => true,
            Err(e) => {
                tracing::error!("mpv render: {e}");
                false
            }
        }
    });
    if painted {
        MPV_DRAWS.fetch_add(1, Ordering::AcqRel);
    } else {
        unsafe { clear_black() };
    }
    if SAMPLING.load(Ordering::Acquire) {
        unsafe { sample(w, h) };
    }
}

/// The brightest channel in a block at the centre of what was just drawn.
unsafe fn sample(w: i32, h: i32) {
    const N: i32 = 32;
    if w < N || h < N {
        return;
    }
    let Some(read) = sym::<extern "C" fn(i32, i32, i32, i32, u32, u32, *mut u8)>(b"glReadPixels\0")
    else {
        return;
    };
    let mut buf = [0u8; (N * N * 4) as usize];
    read(
        (w - N) / 2,
        (h - N) / 2,
        N,
        N,
        GL_RGBA,
        GL_UNSIGNED_BYTE,
        buf.as_mut_ptr(),
    );
    // Alpha is skipped: an opaque black surface has alpha 255.
    let brightest = buf
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|px| px[..3].iter().copied())
        .max()
        .unwrap_or(0);
    PEAK.fetch_max(brightest, Ordering::AcqRel);
}

// ---------------------------------------------------------------------------
// GL, resolved from the framework by name
// ---------------------------------------------------------------------------

fn gl() -> Option<&'static libloading::Library> {
    static LIB: OnceLock<Option<libloading::Library>> = OnceLock::new();
    LIB.get_or_init(|| unsafe {
        libloading::Library::new("/System/Library/Frameworks/OpenGL.framework/OpenGL")
            .map_err(|e| tracing::error!("no OpenGL framework: {e}"))
            .ok()
    })
    .as_ref()
}

/// # Safety
/// `T` must match the real signature of the symbol named by `name`, which must
/// be nul-terminated.
unsafe fn sym<T: Copy>(name: &[u8]) -> Option<T> {
    unsafe { gl()?.get::<T>(name).ok().map(|s| *s) }
}

/// Address lookup for libmpv's render API. A name the framework does not
/// export yields null, which mpv treats as "extension absent".
fn get_proc_address(name: &str) -> *mut c_void {
    let Ok(c) = CString::new(name) else {
        return std::ptr::null_mut();
    };
    unsafe { sym::<*mut c_void>(c.as_bytes_with_nul()) }.unwrap_or(std::ptr::null_mut())
}

unsafe fn get_integer(pname: u32) -> i32 {
    let mut v = [0i32; 1];
    unsafe { get_integers(pname, &mut v) };
    v[0]
}

unsafe fn get_integers(pname: u32, out: &mut [i32]) {
    if let Some(f) = unsafe { sym::<extern "C" fn(u32, *mut i32)>(b"glGetIntegerv\0") } {
        f(pname, out.as_mut_ptr());
    }
}

/// `glGetString`, for the log. Requires a current context.
unsafe fn gl_string(name: u32) -> String {
    let get = unsafe { sym::<extern "C" fn(u32) -> *const std::ffi::c_char>(b"glGetString\0") };
    let Some(get) = get else {
        return "?".to_owned();
    };
    let p = get(name);
    if p.is_null() {
        return "?".to_owned();
    }
    unsafe { std::ffi::CStr::from_ptr(p) }
        .to_string_lossy()
        .into_owned()
}

unsafe fn bind_framebuffer(fbo: i32) {
    if let Some(f) = unsafe { sym::<extern "C" fn(u32, u32)>(b"glBindFramebuffer\0") } {
        f(GL_FRAMEBUFFER, fbo as u32);
    }
}

unsafe fn clear_black() {
    let clear_color = unsafe { sym::<extern "C" fn(f32, f32, f32, f32)>(b"glClearColor\0") };
    let clear = unsafe { sym::<extern "C" fn(u32)>(b"glClear\0") };
    if let (Some(clear_color), Some(clear)) = (clear_color, clear) {
        clear_color(0.0, 0.0, 0.0, 1.0);
        clear(GL_COLOR_BUFFER_BIT);
    }
}

/// `LC_NUMERIC` in macOS's libc.
const LC_NUMERIC: i32 = 4;

/// libmpv refuses to initialise unless `LC_NUMERIC` is `C`. An AppKit app
/// starts in the C locale, but a host that set the user's would make
/// `mpv_create` fail with nothing but a generic error.
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
