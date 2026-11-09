//! Playback surface: where video is drawn, and the only module that knows
//! which windowing toolkit draws it.
//!
//! The rest of the app sees a boundary with no toolkit in it:
//!
//! - [`install`] puts a video surface beneath the main window's webview and
//!   creates the player that draws into it. Once, from `setup`.
//! - [`set_video_rect`] moves that surface; the frontend animates the picture
//!   into a corner and back through it.
//! - [`repeat_on_main`] is a main-thread timer, which is what the player's
//!   heartbeat runs on.
//! - [`request_capture`] is the development self-capture (`ZAPPER_CAPTURE`).
//! - [`mpv`] is the player itself and [`trickplay`] the scrollback
//!   thumbnails; both are portable, and neither names a toolkit. Neither
//!   does [`title_thumbs`], which fills a title's thumbnails from what mpv
//!   has buffered.
//!
//! Behind it, one module per platform, chosen at compile time:
//!
//! - `linux`: GTK. A GtkGLArea under WebKitGTK's webview, mpv rendering into
//!   it through the OpenGL render API. Everything that knows GTK, GL or
//!   WebKit exists lives there and nowhere else.
//! - `windows`: a child window beneath WebView2's, which mpv draws into by
//!   itself (`wid` embedding); the webview is transparent over it.
//! - `macos`: AppKit and Core Animation. An OpenGL layer in a view beneath
//!   the WKWebView, mpv rendering into it through the OpenGL render API as
//!   on Linux, the webview transparent over it as on Windows.
//!
//! Every platform function is main-thread-only, for the same reason on each:
//! the toolkit's objects are not `Send`. Commands on worker threads hop with
//! `AppHandle::run_on_main_thread` and carry plain data across.

pub mod mpv;
pub mod png;
pub mod title_thumbs;
pub mod trickplay;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
use self::windows as platform;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
compile_error!("zapper has a playback surface for Linux, Windows and macOS only");

use std::path::PathBuf;
use std::time::Duration;

/// Puts the video surface beneath `window`'s webview and creates the player
/// drawing into it.
///
/// Failing is fatal to the caller on purpose: a running app with no surface
/// is worse than no app. Must be called on the main thread, which is where
/// Tauri's `setup` hook runs.
pub fn install(window: &tauri::WebviewWindow) -> anyhow::Result<()> {
    platform::install(window)
}

/// Moves the video surface. Main thread only; `Err` before [`install`].
pub fn set_video_rect(rect: VideoRect) -> Result<(), String> {
    platform::set_video_rect(rect)
}

/// Calls `f` on the main thread every `interval` until it returns `false`.
///
/// The toolkit's own timer on each platform, so the callback runs between
/// the event loop's other work and may touch main-thread-only state. Must be
/// called on the main thread.
pub fn repeat_on_main(interval: Duration, f: impl FnMut() -> bool + 'static) {
    platform::repeat_on_main(interval, f)
}

/// Takes the surface and the page down in order for quitting, then calls
/// `then` on the main thread. Main thread only; once.
///
/// On Linux: the window hides, playback stops and mpv's render context is
/// freed while its GL context still exists, the page saves what it keeps
/// (`pagehide`, and the saves it has in flight), and WebKit ends its web
/// process (see `linux::shut_down`). Elsewhere `then` runs straight away.
pub fn shut_down(then: impl FnOnce() + 'static) {
    platform::shut_down(then)
}

/// Writes a reconstruction of the window to `path` after `after`, then exits
/// the process. Development only (`ZAPPER_CAPTURE`); a platform that cannot
/// do it says so and carries on.
pub fn request_capture(path: PathBuf, after: Duration) {
    platform::request_capture(path, after)
}

use serde::{Deserialize, Serialize};

/// Where video should be drawn, in normalised window coordinates.
///
/// Video is not a DOM element and cannot be positioned with CSS, so the
/// frontend sends this and the platform applies it. Docking the video into a corner
/// while browsing is an animation over this rect.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VideoRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl VideoRect {
    pub fn full() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        }
    }

    /// Clamps into the window and never returns a zero-sized viewport, which
    /// GL rejects and a zero-sized window would simply hide.
    pub fn to_pixels(&self, win_w: i32, win_h: i32) -> (i32, i32, i32, i32) {
        let clamp01 = |v: f64| v.clamp(0.0, 1.0);
        let x = (clamp01(self.x) * win_w as f64) as i32;
        let y = (clamp01(self.y) * win_h as f64) as i32;
        let w = ((clamp01(self.x + self.w) * win_w as f64) as i32 - x).max(1);
        let h = ((clamp01(self.y + self.h) * win_h as f64) as i32 - y).max(1);
        (x, y, w, h)
    }
}

impl Default for VideoRect {
    fn default() -> Self {
        Self::full()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullscreen_rect_is_the_default() {
        let r = VideoRect::full();
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 0.0, 1.0, 1.0));
        assert_eq!(VideoRect::default(), r);
    }

    #[test]
    fn rect_scales_to_pixels() {
        let r = VideoRect {
            x: 0.5,
            y: 0.5,
            w: 0.5,
            h: 0.5,
        };
        assert_eq!(r.to_pixels(1000, 800), (500, 400, 500, 400));
    }

    #[test]
    fn rect_is_clamped_to_the_window() {
        let r = VideoRect {
            x: -0.5,
            y: 0.0,
            w: 2.0,
            h: 1.0,
        };
        let (x, _, w, _) = r.to_pixels(1000, 800);
        assert_eq!((x, w), (0, 1000));
    }

    #[test]
    fn degenerate_rect_never_yields_zero_size() {
        let r = VideoRect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        };
        let (_, _, w, h) = r.to_pixels(1000, 800);
        assert!(w >= 1 && h >= 1, "GL would fail on a zero-sized viewport");
    }
}
