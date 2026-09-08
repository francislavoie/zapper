//! Playback surface: the only module aware that GTK exists.

pub mod gl;
pub mod mpv;
pub mod overlay;

use serde::{Deserialize, Serialize};

/// Where video should be drawn, in normalised window coordinates.
///
/// Video is not a DOM element and cannot be positioned with CSS, so the
/// frontend sends this and GTK applies it. Docking the video into a corner
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
    /// GL rejects.
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

use std::cell::RefCell;

thread_local! {
    /// The installed overlay, reachable only from the GTK main thread.
    ///
    /// It cannot live in Tauri's managed state: it holds `Rc` and GTK objects,
    /// so it is neither `Send` nor `Sync`. Commands arriving on other threads
    /// must hop across with `AppHandle::run_on_main_thread` before calling
    /// [`with_overlay`].
    static INSTALLED: RefCell<Option<overlay::Overlay>> = const { RefCell::new(None) };
}

/// Records the overlay for later command access. Call once, from `setup`.
pub fn set_installed(o: overlay::Overlay) {
    INSTALLED.with(|i| *i.borrow_mut() = Some(o));
}

/// Runs `f` against the overlay. Returns `None` off the GTK main thread or
/// before installation.
pub fn with_overlay<R>(f: impl FnOnce(&overlay::Overlay) -> R) -> Option<R> {
    INSTALLED.with(|i| i.borrow().as_ref().map(f))
}
