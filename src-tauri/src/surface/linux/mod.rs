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
