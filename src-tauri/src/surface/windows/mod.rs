//! The Windows surface: a child window beneath WebView2, which mpv draws
//! into by itself.
//!
//! Linux has to render mpv's frames itself, into a GL surface it slides under
//! the webview, because WebKitGTK gives it nothing else to stand on. Windows
//! has a simpler arrangement available and this uses it:
//!
//! - A plain child window (`ZAPPER_VIDEO`) is created inside Tauri's main
//!   window and pushed to the **bottom** of its siblings. wry's WebView2
//!   container is a sibling it places at the top, so the page is above the
//!   video and every click lands on the page.
//! - mpv is told to draw into that window (`wid`), with its own `gpu-next`
//!   renderer on Direct3D 11. Nothing here touches a frame: no render
//!   context, no framebuffer, no draw callback.
//! - The webview is made transparent (WebView2's `DefaultBackgroundColor`
//!   with alpha 0, through Tauri), so wherever the page has no background of
//!   its own the video shows through it — the same arrangement as on Linux,
//!   composited by DWM instead of GTK.
//!
//! The child window never takes input: it is created disabled, answers every
//! hit test with "transparent", and refuses activation. mpv is configured to
//! ignore the keyboard and mouse as well, so that if a click ever did reach
//! it, it could not pause or seek behind the interface's back.
//!
//! [`set_video_rect`] is a `SetWindowPos` on that window, in the main
//! window's physical pixels, and is reapplied whenever the main window
//! changes size (including a DPI change, which arrives as a resize).
//!
//! Scrollback thumbnails cannot be copied out of a framebuffer here, because
//! there is none of ours: mpv's `screenshot-raw` provides the current frame
//! instead, on a worker thread, and it is shrunk on the CPU (see
//! [`crate::surface::trickplay::shrink_bgr0`]).
//!
//! Everything here is main-thread-only: window procedures and thread timers
//! run on the thread that created them, which is the one Tauri's `setup`
//! hook runs on.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ::windows::core::{w, PCWSTR};
use ::windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use ::windows::Win32::Graphics::Gdi::{GetStockObject, BLACK_BRUSH, HBRUSH};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use ::windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, GetWindow, KillTimer, LoadCursorW,
    RegisterClassExW, SetTimer, SetWindowPos, CS_HREDRAW, CS_VREDRAW, GW_CHILD, GW_HWNDNEXT,
    HTTRANSPARENT, HWND_BOTTOM, IDC_ARROW, MA_NOACTIVATE, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
    SWP_NOZORDER, WINDOW_EX_STYLE, WM_DPICHANGED, WM_MOUSEACTIVATE, WM_NCHITTEST, WM_SIZE,
    WNDCLASSEXW, WS_CHILD, WS_CLIPCHILDREN, WS_DISABLED, WS_VISIBLE,
};

use crate::surface::{mpv, trickplay, VideoRect};

/// The video window's class name.
const CLASS: PCWSTR = w!("ZAPPER_VIDEO");

/// Our subclass of the main window, told apart from wry's (which subclasses
/// the same window to resize the webview) by this id.
const SUBCLASS_ID: usize = 0x7a61_7070; // "zapp"

/// How often the thumbnail clock is looked at. The clock itself decides
/// when one is due (every few seconds of stream); this only bounds how late
/// that can be noticed.
const THUMB_POLL: Duration = Duration::from_secs(1);

thread_local! {
    /// The main window and the video window inside it.
    static SURFACE: Cell<Option<(HWND, HWND)>> = const { Cell::new(None) };
    /// Where the video goes, normalised. Kept so a resize can reapply it.
    static PLACED: Cell<VideoRect> = Cell::new(VideoRect::full());
    /// Callbacks for [`repeat_on_main`], by timer id.
    static TIMERS: RefCell<HashMap<usize, Box<dyn FnMut() -> bool>>> =
        RefCell::new(HashMap::new());
}

pub(super) fn install(window: &tauri::WebviewWindow) -> anyhow::Result<()> {
    use anyhow::Context;

    let parent = window.hwnd().context("the main window has no HWND")?;

    // The page must not paint an opaque white sheet over the video: WebView2
    // fills its background before the page draws, and alpha 0 is the one
    // translucent value it accepts. Webview only — the window under it is
    // painted black, which is what shows where the video does not reach (the
    // instant before the first frame, a docked picture's surroundings).
    let webview: &tauri::Webview = window.as_ref();
    webview
        .set_background_color(Some(tauri::window::Color(0, 0, 0, 0)))
        .context("making the webview transparent")?;
    webview
        .window()
        .set_background_color(Some(tauri::window::Color(0, 0, 0, 255)))
        .context("painting the window black")?;

    let child = create_video_window(parent)?;
    SURFACE.with(|s| s.set(Some((parent, child))));
    place();

    // Follow the main window's size. A subclass rather than Tauri's resize
    // event, for the same reason wry uses one for the webview: it runs inside
    // WM_SIZE itself, including during the modal sizing loop, so the video
    // and the page move in the same frame rather than one event apart.
    let ok = unsafe { SetWindowSubclass(parent, Some(follow_parent), SUBCLASS_ID, 0) };
    if !ok.as_bool() {
        tracing::warn!("could not subclass the main window; the video will not follow resizes");
    }

    // HWNDs are pointer-sized integers to mpv. The handle is the window's for
    // as long as it exists, which is the life of the process.
    let wid = child.0 as i64;
    mpv::Player::new(|init| {
        init.set_property("vo", "gpu-next".to_owned())?;
        init.set_property("gpu-api", "d3d11".to_owned())?;
        init.set_property("wid", wid)?;
        // mpv's own input handling is for mpv the application. Here the
        // interface owns the keyboard and mouse, and a stray key reaching
        // mpv's window must not pause or seek behind its back.
        init.set_property("input-default-bindings", false)?;
        init.set_property("input-vo-keyboard", false)?;
        init.set_property("input-cursor", false)?;
        init.set_property("cursor-autohide", "no".to_owned())?;
        Ok(())
    })?;

    repeat_on_main(THUMB_POLL, || {
        thumbnail_tick();
        true
    });
    tracing::info!("video surface: mpv window embedded beneath the webview");
    Ok(())
}

/// Registers the class and creates the video window, at the bottom of the
/// main window's children.
fn create_video_window(parent: HWND) -> anyhow::Result<HWND> {
    use anyhow::Context;

    let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }
        .context("GetModuleHandleW")?
        .into();
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(video_wndproc),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        // Black before mpv has drawn anything, and wherever it is not
        // drawing: the same as the window behind it.
        hbrBackground: HBRUSH(unsafe { GetStockObject(BLACK_BRUSH) }.0),
        lpszClassName: CLASS,
        ..Default::default()
    };
    // Fails harmlessly if already registered; CreateWindowExW is what says
    // whether the class is usable.
    unsafe { RegisterClassExW(&class) };

    let child = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            PCWSTR::null(),
            // Disabled: a disabled window receives no keyboard or mouse
            // input, which is the whole of this window's relationship with
            // the user. No WS_CLIPSIBLINGS: the webview above it covers the
            // whole window, and clipping against it would clip everything.
            WS_CHILD | WS_VISIBLE | WS_DISABLED | WS_CLIPCHILDREN,
            0,
            0,
            1,
            1,
            Some(parent),
            None,
            Some(instance),
            None,
        )
    }
    .context("creating the video window")?;
    Ok(child)
}

/// The video window's procedure: default behaviour, minus any claim on
/// input, plus keeping mpv's own window the size of ours.
unsafe extern "system" fn video_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        // Belt and braces with WS_DISABLED: a hit test that says
        // "transparent" sends the mouse to whatever is beneath, and an
        // activation refusal stops a click from focusing this window.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_SIZE => {
            // mpv's window is a child of this one, owned by mpv's own thread.
            // mpv follows its parent's size on its own, but only when it next
            // looks; resizing it here means a dock animation never shows a
            // frame of mismatched size. Asynchronous, because a synchronous
            // SetWindowPos on another thread's window waits for that thread.
            let (w, h) = ((lparam.0 & 0xffff) as i32, ((lparam.0 >> 16) & 0xffff) as i32);
            let mut c = unsafe { GetWindow(hwnd, GW_CHILD) }.ok();
            while let Some(child) = c {
                let _ = unsafe {
                    SetWindowPos(
                        child,
                        None,
                        0,
                        0,
                        w,
                        h,
                        SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
                    )
                };
                c = unsafe { GetWindow(child, GW_HWNDNEXT) }.ok();
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// The main window's subclass: reapply the video rect after every resize.
unsafe extern "system" fn follow_parent(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // The default first, so wry's own subclass has sized the webview and the
    // client rect is the new one by the time the video is placed.
    let r = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    if msg == WM_SIZE || msg == WM_DPICHANGED {
        place();
    }
    r
}

/// Puts the video window where [`PLACED`] says, in the main window's
/// current physical pixels, and back at the bottom of the stack.
fn place() {
    let Some((parent, child)) = SURFACE.with(Cell::get) else {
        return;
    };
    let mut client = RECT::default();
    if unsafe { GetClientRect(parent, &mut client) }.is_err() {
        return;
    }
    let (w, h) = (client.right - client.left, client.bottom - client.top);
    if w <= 0 || h <= 0 {
        // Minimised. Keep the last placement rather than collapse to a pixel.
        return;
    }
    let (x, y, vw, vh) = PLACED.with(Cell::get).to_pixels(w, h);
    // HWND_BOTTOM every time, not only at creation: it costs nothing and
    // means nothing that reorders siblings later can put video over the page.
    if let Err(e) =
        unsafe { SetWindowPos(child, Some(HWND_BOTTOM), x, y, vw, vh, SWP_NOACTIVATE) }
    {
        tracing::warn!("placing the video window: {e}");
    }
}

pub(super) fn set_video_rect(rect: VideoRect) -> Result<(), String> {
    if SURFACE.with(Cell::get).is_none() {
        return Err("the video surface is not installed".to_owned());
    }
    PLACED.with(|p| p.set(rect));
    place();
    Ok(())
}

/// A thread timer (`SetTimer` with no window) whose callback is looked up by
/// id. Tao's event loop dispatches `WM_TIMER` like any other message, and so
/// do the modal loops Windows runs while a window is being dragged or sized,
/// so the heartbeat keeps going through those.
pub(super) fn repeat_on_main(interval: Duration, f: impl FnMut() -> bool + 'static) {
    let ms = interval.as_millis().clamp(1, u128::from(u32::MAX)) as u32;
    let id = unsafe { SetTimer(None, 0, ms, Some(on_timer)) };
    if id == 0 {
        tracing::error!("SetTimer failed; a main-thread timer will not run");
        return;
    }
    TIMERS.with(|t| t.borrow_mut().insert(id, Box::new(f)));
}

unsafe extern "system" fn on_timer(_hwnd: HWND, _msg: u32, id: usize, _time: u32) {
    // Taken out while it runs, so a callback that starts another timer does
    // not find the map already borrowed.
    let Some(mut f) = TIMERS.with(|t| t.borrow_mut().remove(&id)) else {
        return;
    };
    if f() {
        TIMERS.with(|t| t.borrow_mut().insert(id, f));
    } else {
        let _ = unsafe { KillTimer(None, id) };
    }
}

/// mpv draws into a window it was given before any file opened, so there is
/// nothing to wait for.
pub(super) fn video_ready() -> bool {
    true
}

pub(super) fn request_frame() {}

/// The development capture reconstructs GTK's two layers and has no Windows
/// counterpart yet.
pub(super) fn request_capture(_path: PathBuf, _after: Duration) {
    eprintln!("CAPTURE FAILED: ZAPPER_CAPTURE is only implemented on Linux");
}

// ---------------------------------------------------------------------------
// Scrollback thumbnails
// ---------------------------------------------------------------------------

/// One screenshot at a time. `screenshot-raw` blocks its caller until mpv
/// has the frame; a slow one must not queue another behind it.
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Starts a thumbnail if one is due. On the main thread, from the timer.
fn thumbnail_tick() {
    // Nothing loaded, nothing to photograph. Read before the clock, so that
    // an idle player does not use up the due time.
    let Some(Some(at)) = mpv::with_player(|p| p.get_f64("playback-time")) else {
        return;
    };
    if !trickplay::is_due() || IN_FLIGHT.swap(true, Ordering::AcqRel) {
        return;
    }
    let generation = trickplay::generation();
    // Off the main thread: the screenshot waits on mpv's core, and the
    // interface must not wait with it.
    let spawned = std::thread::Builder::new()
        .name("zapper-thumb".into())
        .spawn(move || {
            if let Some(rgba) = screenshot_thumbnail() {
                trickplay::record_for(generation, at, rgba);
            }
            IN_FLIGHT.store(false, Ordering::Release);
        });
    if spawned.is_err() {
        IN_FLIGHT.store(false, Ordering::Release);
    }
}

/// mpv's current video frame, shrunk to a thumbnail. `None` when mpv has no
/// frame (between channels) or hands back a format this does not read.
fn screenshot_thumbnail() -> Option<Vec<u8>> {
    use libmpv2_sys::{
        mpv_command_ret, mpv_format_MPV_FORMAT_NODE_MAP, mpv_free_node_contents, mpv_node,
    };

    let handle = mpv::handle()?;
    // `video`: the decoded picture at its own size, without subtitles or
    // OSD, which is what a thumbnail is of. The format is left to mpv's
    // default, `bgr0`, which every version that has `screenshot-raw` speaks.
    let mut args = [c"screenshot-raw".as_ptr(), c"video".as_ptr(), std::ptr::null()];
    let mut node: mpv_node = unsafe { std::mem::zeroed() };
    let rc = unsafe { mpv_command_ret(handle.ctx.as_ptr(), args.as_mut_ptr(), &mut node) };
    if rc < 0 {
        return None;
    }
    let out = if node.format == mpv_format_MPV_FORMAT_NODE_MAP {
        unsafe { shrink_node(&node) }
    } else {
        None
    };
    unsafe { mpv_free_node_contents(&mut node) };
    out
}

/// Reads `screenshot-raw`'s map — `w`, `h`, `stride`, `format`, `data` — and
/// shrinks the frame it describes.
///
/// # Safety
/// `node` must be a `MPV_FORMAT_NODE_MAP` returned by mpv and not yet freed.
unsafe fn shrink_node(node: &libmpv2_sys::mpv_node) -> Option<Vec<u8>> {
    use libmpv2_sys::{
        mpv_format_MPV_FORMAT_BYTE_ARRAY, mpv_format_MPV_FORMAT_INT64,
        mpv_format_MPV_FORMAT_STRING,
    };
    use std::ffi::CStr;

    let list = unsafe { node.u.list.as_ref() }?;
    let (mut w, mut h, mut stride) = (None, None, None);
    let (mut format, mut data) = (None, None);
    for i in 0..usize::try_from(list.num).ok()? {
        let key = unsafe { CStr::from_ptr(*list.keys.add(i)) }.to_bytes();
        let value = unsafe { &*list.values.add(i) };
        match (key, value.format) {
            (b"w", f) if f == mpv_format_MPV_FORMAT_INT64 => w = Some(unsafe { value.u.int64 }),
            (b"h", f) if f == mpv_format_MPV_FORMAT_INT64 => h = Some(unsafe { value.u.int64 }),
            (b"stride", f) if f == mpv_format_MPV_FORMAT_INT64 => {
                stride = Some(unsafe { value.u.int64 })
            }
            (b"format", f) if f == mpv_format_MPV_FORMAT_STRING => {
                format = Some(unsafe { CStr::from_ptr(value.u.string) }.to_bytes().to_vec())
            }
            (b"data", f) if f == mpv_format_MPV_FORMAT_BYTE_ARRAY => {
                data = unsafe { value.u.ba.as_ref() }
            }
            _ => {}
        }
    }
    if format.as_deref() != Some(b"bgr0".as_slice()) {
        return None;
    }
    let (w, h, stride) = (
        usize::try_from(w?).ok()?,
        usize::try_from(h?).ok()?,
        usize::try_from(stride?).ok()?,
    );
    let data = data?;
    if data.data.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data.data as *const u8, data.size) };
    trickplay::shrink_bgr0(bytes, w, h, stride)
}
