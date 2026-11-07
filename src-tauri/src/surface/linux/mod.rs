//! The Linux surface: GTK, WebKitGTK and OpenGL.
//!
//! The only place in the program that knows GTK exists. A GtkGLArea is slid
//! underneath Tauri's webview ([`overlay`]), mpv renders into it through its
//! OpenGL render API ([`render`]), and the frame it has just drawn is where
//! scrollback thumbnails ([`thumbs`]) and the development capture
//! ([`capture`]) come from. GL itself is resolved by hand in [`gl`].
//!
//! What the rest of the app may call is the boundary in `surface/mod.rs`; the
//! modules here are public only so the GTK verification harnesses
//! (`src/bin/*_check.rs`) can reach the widgets directly.

pub mod capture;
pub mod gl;
pub mod overlay;
pub mod render;
pub mod thumbs;

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

use gtk::prelude::GLAreaExt;

use crate::surface::VideoRect;

thread_local! {
    /// The installed overlay, reachable only from the GTK main thread.
    ///
    /// It cannot live in Tauri's managed state: it holds `Rc` and GTK objects,
    /// so it is neither `Send` nor `Sync`. Commands arriving on other threads
    /// must hop across with `AppHandle::run_on_main_thread` before calling
    /// [`with_overlay`].
    static INSTALLED: RefCell<Option<overlay::Overlay>> = const { RefCell::new(None) };
}

/// Records the overlay for later command access.
pub fn set_installed(o: overlay::Overlay) {
    INSTALLED.with(|i| *i.borrow_mut() = Some(o));
}

/// Runs `f` against the overlay. Returns `None` off the GTK main thread or
/// before installation.
pub fn with_overlay<R>(f: impl FnOnce(&overlay::Overlay) -> R) -> Option<R> {
    INSTALLED.with(|i| i.borrow().as_ref().map(f))
}

pub(super) fn install(window: &tauri::WebviewWindow) -> anyhow::Result<()> {
    let overlay = overlay::install(window)?;
    render::new_player(&overlay.gl_area)?;
    // Not app.manage(): the overlay holds Rc and GTK objects, so it is
    // !Send. It lives in a GTK-main-thread local instead.
    set_installed(overlay);
    Ok(())
}

/// How long the page may take to unload, and then its process to end, before
/// quitting goes ahead without it. Each normally takes a few milliseconds.
const QUIT_STEP: Duration = Duration::from_millis(1500);

/// Ends playback, then the page, then WebKit's web process, and calls `then`
/// once they are gone. See [`crate::surface::shut_down`].
///
/// The window is hidden as soon as the video is down, so the close looks
/// immediate: it stays realized, so nothing about it is torn down early, and
/// the page sees `visibilitychange` to hidden. The page is unloaded first (`about:blank`), so it gets its `pagehide` and
/// writes what it keeps in local storage while the app is still there. The
/// web process is then ended through WebKit's own API rather than left to
/// notice that the app has gone and exit by itself. Exiting by itself, it
/// runs its libraries' exit handlers while its own threads may still be
/// letting go of their EGL state, and libEGL's handler has been seen to crash
/// there: heap corruption found by `free` under `_dl_fini`, with another
/// WebKit thread inside Mesa's EGL in its thread-exit destructors. The
/// desktop reports that as a crash. A process WebKit ends runs no exit
/// handlers at all.
pub(super) fn shut_down(then: impl FnOnce() + 'static) {
    use gtk::prelude::WidgetExt;

    render::shut_down();
    match with_overlay(|o| o.webview.upgrade()).flatten() {
        Some(webview) => {
            if let Some(window) = webview.toplevel() {
                window.hide();
            }
            end_page(webview, Box::new(then))
        }
        None => then(),
    }
}

/// Once-only continuation shared by a signal handler and its timeout.
type Step = std::rc::Rc<RefCell<Option<Box<dyn FnOnce()>>>>;

fn once(f: Box<dyn FnOnce()>) -> Step {
    std::rc::Rc::new(RefCell::new(Some(f)))
}

fn advance(step: &Step) {
    let f = step.borrow_mut().take();
    if let Some(f) = f {
        f();
    }
}

fn end_page(webview: webkit2gtk::WebView, then: Box<dyn FnOnce()>) {
    use gtk::glib::object::ObjectExt;
    use webkit2gtk::{LoadEvent, WebViewExt};

    let handler: std::rc::Rc<RefCell<Option<gtk::glib::SignalHandlerId>>> = Default::default();
    let next = {
        let (webview, handler) = (webview.clone(), handler.clone());
        once(Box::new(move || {
            if let Some(id) = handler.borrow_mut().take() {
                webview.disconnect(id);
            }
            end_web_process(webview, then);
        }))
    };
    let on_load = next.clone();
    *handler.borrow_mut() = Some(webview.connect_load_changed(move |v, event| {
        if event == LoadEvent::Finished && v.uri().is_some_and(|u| u == "about:blank") {
            advance(&on_load);
        }
    }));
    let late = next.clone();
    gtk::glib::timeout_add_local_once(QUIT_STEP, move || {
        if late.borrow().is_some() {
            tracing::warn!("the page did not unload in time; quitting anyway");
        }
        advance(&late);
    });
    webview.load_uri("about:blank");
}

fn end_web_process(webview: webkit2gtk::WebView, then: Box<dyn FnOnce()>) {
    use webkit2gtk::WebViewExt;

    let done = once(then);
    let on_end = done.clone();
    webview.connect_web_process_terminated(move |_, reason| {
        tracing::info!("web process ended: {reason:?}");
        advance(&on_end)
    });
    let late = done.clone();
    gtk::glib::timeout_add_local_once(QUIT_STEP, move || {
        if late.borrow().is_some() {
            tracing::warn!("the web process did not end in time; quitting anyway");
        }
        advance(&late);
    });
    webview.terminate_web_process();
}

pub(super) fn set_video_rect(rect: VideoRect) -> Result<(), String> {
    with_overlay(|o| o.set_video_rect(rect))
        .ok_or_else(|| "the video surface is not installed".to_owned())
}

pub(super) fn repeat_on_main(interval: Duration, mut f: impl FnMut() -> bool + 'static) {
    gtk::glib::timeout_add_local(interval, move || {
        if f() {
            gtk::glib::ControlFlow::Continue
        } else {
            gtk::glib::ControlFlow::Break
        }
    });
}

pub(super) fn video_ready() -> bool {
    render::ready()
}

pub(super) fn request_frame() {
    render::kick();
}

/// Reconstructs the window into `path` after `after`, prints where, and
/// exits. See [`capture`] for why it is a reconstruction.
pub(super) fn request_capture(path: PathBuf, after: Duration) {
    gtk::glib::timeout_add_seconds_local_once(after.as_secs() as u32, move || {
        capture::request(&path);
        // The surface only redraws when mpv has a new frame, which is
        // right for an idle GPU and wrong for a capture: with nothing
        // playing the render handler would never run and the grab would
        // never happen. Nudge it for a second.
        let mut nudges = 0;
        gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
            with_overlay(|ov| ov.gl_area.queue_render());
            nudges += 1;
            if nudges < 20 {
                gtk::glib::ControlFlow::Continue
            } else {
                gtk::glib::ControlFlow::Break
            }
        });
        gtk::glib::timeout_add_seconds_local_once(2, move || {
            match with_overlay(capture::finish) {
                Some(Ok(p)) => println!("CAPTURED {}", p.display()),
                Some(Err(e)) => eprintln!("CAPTURE FAILED: {e}"),
                None => eprintln!("CAPTURE FAILED: no overlay on this thread"),
            }
            let _ = std::io::Write::flush(&mut std::io::stdout());
            std::process::exit(0);
        });
    });
}
