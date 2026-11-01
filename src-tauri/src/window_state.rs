//! Reopening the window the size it was left at.
//!
//! Size and maximised state come back everywhere. Position comes back only
//! where the app is allowed to place its own window: on Wayland the
//! compositor decides, `set_position` is ignored, and the position a client
//! reads back is always 0,0 — so remembering it would only ever restore a lie.
//!
//! A saved size is a request, not a promise. It is clamped to the work area
//! of the monitor the window will open on, because a window remembered from a
//! 4K desktop and reopened on a laptop panel would otherwise open larger than
//! the screen, with its edges — the only way to resize an undecorated window —
//! out of reach.
//!
//! Kept in a file of its own beside the account rather than in the settings
//! row in SQLite: it is written on every resize, and has nothing to do with
//! the catalogue.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What is remembered, in logical pixels so a change of scale between
/// sessions does not change the window's apparent size.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    #[serde(default)]
    pub maximized: bool,
}

/// A rectangle in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn overlap(&self, o: &Rect) -> f64 {
        let w = (self.x + self.width).min(o.x + o.width) - self.x.max(o.x);
        let h = (self.y + self.height).min(o.y + o.height) - self.y.max(o.y);
        if w > 0.0 && h > 0.0 {
            w * h
        } else {
            0.0
        }
    }
}

/// Where and how big to open.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub width: f64,
    pub height: f64,
    pub position: Option<(f64, f64)>,
    pub maximized: bool,
}

/// The window's floor, from tauri.conf.json. Clamping below it would only be
/// overridden by the toolkit anyway.
const MIN_WIDTH: f64 = 900.0;
const MIN_HEIGHT: f64 = 560.0;

/// A remembered position is used only if at least this much of the window
/// would land on some monitor; otherwise it opens where the system puts it.
const MIN_VISIBLE: f64 = 0.5;

/// Decides where the window opens. `work_areas` are the monitors' usable
/// areas (minus panels), the one the window will open on first.
pub fn placement(saved: &Saved, work_areas: &[Rect], can_position: bool) -> Option<Placement> {
    let sane = |v: f64| v.is_finite() && v >= 1.0;
    if !sane(saved.width) || !sane(saved.height) {
        return None;
    }
    let (mut width, mut height) = (saved.width, saved.height);
    if let Some(area) = work_areas.first() {
        // Never larger than the screen, and never smaller than the window's
        // own minimum — a work area smaller than that is a screen the app
        // does not fit on, and the toolkit's minimum wins regardless.
        width = width.min(area.width).max(MIN_WIDTH.min(area.width));
        height = height.min(area.height).max(MIN_HEIGHT.min(area.height));
    }
    let position = match (saved.x, saved.y) {
        (Some(x), Some(y)) if can_position && x.is_finite() && y.is_finite() => {
            let rect = Rect { x, y, width, height };
            let visible: f64 = work_areas.iter().map(|a| rect.overlap(a)).sum();
            (visible >= MIN_VISIBLE * width * height).then_some((x, y))
        }
        _ => None,
    };
    Some(Placement {
        width,
        height,
        position,
        maximized: saved.maximized,
    })
}

/// What the window looks like right now, for [`next_saved`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observed {
    pub width: f64,
    pub height: f64,
    pub position: Option<(f64, f64)>,
    pub maximized: bool,
    pub minimized: bool,
    pub fullscreen: bool,
}

/// What to remember, given what was remembered and what the window is now.
///
/// While maximised or fullscreen the window's size is the screen's, not one
/// anybody chose, so the last ordinary size is kept and only the flag
/// changes. Fullscreen is not remembered at all: it is something you do to a
/// channel, not to the app. Minimised is ignored outright — the size read
/// back from a minimised window is meaningless on some platforms.
pub fn next_saved(previous: Option<Saved>, now: Observed) -> Option<Saved> {
    if now.minimized {
        return previous;
    }
    if now.fullscreen {
        return previous;
    }
    if now.maximized {
        return previous.map(|p| Saved {
            maximized: true,
            ..p
        });
    }
    if !(now.width >= 1.0 && now.height >= 1.0) {
        return previous;
    }
    Some(Saved {
        width: now.width,
        height: now.height,
        x: now.position.map(|p| p.0),
        y: now.position.map(|p| p.1),
        maximized: false,
    })
}

/// Whether this process can place its own window.
///
/// GDK picks Wayland whenever a Wayland display is present unless told
/// otherwise; `GDK_BACKEND` overrides it, and its first entry is the one
/// tried first.
pub fn can_self_position() -> bool {
    if !cfg!(target_os = "linux") {
        return true;
    }
    let backend = std::env::var("GDK_BACKEND").unwrap_or_default();
    if let Some(first) = backend.split(',').map(str::trim).find(|s| !s.is_empty()) {
        return first == "x11";
    }
    std::env::var("WAYLAND_DISPLAY")
        .map(|s| s.is_empty())
        .unwrap_or(true)
}

fn path() -> PathBuf {
    crate::catalog::credentials::account_path().with_file_name("window.json")
}

fn load() -> Option<Saved> {
    let raw = std::fs::read_to_string(path()).ok()?;
    serde_json::from_str(&raw).ok()
}

fn store(saved: &Saved) {
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Written beside and renamed over, so a crash mid-write leaves the last
    // good size rather than a truncated file that restores nothing.
    let tmp = p.with_extension("json.tmp");
    let Ok(json) = serde_json::to_string(saved) else {
        return;
    };
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// Saving is debounced: a drag-resize emits an event per frame.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

fn logical_rect(pos: tauri::PhysicalPosition<i32>, size: tauri::PhysicalSize<u32>, scale: f64) -> Rect {
    Rect {
        x: f64::from(pos.x) / scale,
        y: f64::from(pos.y) / scale,
        width: f64::from(size.width) / scale,
        height: f64::from(size.height) / scale,
    }
}

fn work_areas(window: &tauri::WebviewWindow) -> Vec<Rect> {
    let to_rect = |m: &tauri::Monitor| {
        let wa = m.work_area();
        logical_rect(wa.position, wa.size, m.scale_factor())
    };
    let mut areas = Vec::new();
    // The monitor the window is on first, since that is what the size is
    // clamped to; then every monitor, for the visibility check.
    if let Some(m) = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())
    {
        areas.push(to_rect(&m));
    }
    if let Ok(all) = window.available_monitors() {
        areas.extend(all.iter().map(to_rect));
    }
    areas
}

fn observe(window: &tauri::WebviewWindow, can_position: bool) -> Option<Observed> {
    let scale = window.scale_factor().ok()?;
    let size = window.inner_size().ok()?;
    let position = if can_position {
        window
            .outer_position()
            .ok()
            .map(|p| (f64::from(p.x) / scale, f64::from(p.y) / scale))
    } else {
        None
    };
    Some(Observed {
        width: f64::from(size.width) / scale,
        height: f64::from(size.height) / scale,
        position,
        maximized: window.is_maximized().unwrap_or(false),
        minimized: window.is_minimized().unwrap_or(false),
        fullscreen: window.is_fullscreen().unwrap_or(false),
    })
}

/// Restores the remembered size, then keeps remembering it.
///
/// Called from `setup`, before the event loop has run, so the window has not
/// been shown at its configured size yet and the restore causes no visible
/// jump.
pub fn install(window: &tauri::WebviewWindow) {
    let can_position = can_self_position();
    let saved = load();

    if let Some(p) = saved.and_then(|s| placement(&s, &work_areas(window), can_position)) {
        let _ = window.set_size(tauri::LogicalSize::new(p.width, p.height));
        if let Some((x, y)) = p.position {
            let _ = window.set_position(tauri::LogicalPosition::new(x, y));
        }
        if p.maximized {
            let _ = window.maximize();
        }
    }

    let remembered = Arc::new(Mutex::new(saved));
    let generation = Arc::new(AtomicU64::new(0));
    let w = window.clone();
    window.on_window_event(move |event| {
        use tauri::WindowEvent;
        match event {
            WindowEvent::Resized(_)
            | WindowEvent::Moved(_)
            | WindowEvent::ScaleFactorChanged { .. } => {
                let mine = generation.fetch_add(1, Ordering::SeqCst) + 1;
                let generation = generation.clone();
                let remembered = remembered.clone();
                let w = w.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(SAVE_DEBOUNCE).await;
                    if generation.load(Ordering::SeqCst) == mine {
                        // Window queries belong on the main thread.
                        let w2 = w.clone();
                        let _ = w.run_on_main_thread(move || {
                            save_now(&w2, can_position, &remembered)
                        });
                    }
                });
            }
            WindowEvent::CloseRequested { .. } => {
                generation.fetch_add(1, Ordering::SeqCst);
                save_now(&w, can_position, &remembered);
            }
            _ => {}
        }
    });
}

fn save_now(w: &tauri::WebviewWindow, can_position: bool, remembered: &Mutex<Option<Saved>>) {
    let Some(now) = observe(w, can_position) else {
        return;
    };
    let mut slot = remembered.lock().unwrap_or_else(|e| e.into_inner());
    let next = next_saved(*slot, now);
    if next != *slot {
        if let Some(s) = next {
            store(&s);
        }
        *slot = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP: Rect = Rect {
        x: 0.0,
        y: 32.0,
        width: 1504.0,
        height: 970.0,
    };

    fn saved(w: f64, h: f64) -> Saved {
        Saved {
            width: w,
            height: h,
            x: None,
            y: None,
            maximized: false,
        }
    }

    #[test]
    fn a_size_from_a_bigger_screen_is_clamped_to_this_one() {
        let p = placement(&saved(2400.0, 1400.0), &[LAPTOP], false).unwrap();
        assert_eq!((p.width, p.height), (1504.0, 970.0));
    }

    #[test]
    fn a_size_that_fits_comes_back_exactly() {
        let p = placement(&saved(1200.0, 800.0), &[LAPTOP], false).unwrap();
        assert_eq!((p.width, p.height), (1200.0, 800.0));
    }

    #[test]
    fn nothing_is_restored_below_the_window_minimum() {
        let p = placement(&saved(300.0, 200.0), &[LAPTOP], false).unwrap();
        assert_eq!((p.width, p.height), (MIN_WIDTH, MIN_HEIGHT));
    }

    #[test]
    fn garbage_restores_nothing() {
        assert!(placement(&saved(f64::NAN, 800.0), &[LAPTOP], false).is_none());
        assert!(placement(&saved(0.0, 800.0), &[LAPTOP], false).is_none());
    }

    #[test]
    fn position_is_ignored_where_the_compositor_places_windows() {
        let s = Saved {
            x: Some(100.0),
            y: Some(100.0),
            ..saved(1200.0, 800.0)
        };
        assert_eq!(placement(&s, &[LAPTOP], false).unwrap().position, None);
        assert_eq!(
            placement(&s, &[LAPTOP], true).unwrap().position,
            Some((100.0, 100.0))
        );
    }

    #[test]
    fn a_position_on_a_monitor_that_is_gone_is_dropped() {
        let s = Saved {
            x: Some(3000.0),
            y: Some(100.0),
            ..saved(1200.0, 800.0)
        };
        assert_eq!(placement(&s, &[LAPTOP], true).unwrap().position, None);
    }

    #[test]
    fn maximised_keeps_the_last_ordinary_size() {
        let before = Some(saved(1200.0, 800.0));
        let now = Observed {
            width: 1504.0,
            height: 970.0,
            position: None,
            maximized: true,
            minimized: false,
            fullscreen: false,
        };
        let next = next_saved(before, now).unwrap();
        assert!(next.maximized);
        assert_eq!((next.width, next.height), (1200.0, 800.0));
    }

    #[test]
    fn fullscreen_and_minimised_are_not_remembered() {
        let before = Some(saved(1200.0, 800.0));
        let base = Observed {
            width: 3000.0,
            height: 2000.0,
            position: None,
            maximized: false,
            minimized: false,
            fullscreen: true,
        };
        assert_eq!(next_saved(before, base), before);
        let min = Observed {
            fullscreen: false,
            minimized: true,
            width: 0.0,
            height: 0.0,
            ..base
        };
        assert_eq!(next_saved(before, min), before);
    }

    #[test]
    fn an_ordinary_resize_is_remembered() {
        let now = Observed {
            width: 1000.0,
            height: 700.0,
            position: None,
            maximized: false,
            minimized: false,
            fullscreen: false,
        };
        assert_eq!(next_saved(None, now), Some(saved(1000.0, 700.0)));
    }
}
