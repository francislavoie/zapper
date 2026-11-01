//! The window's chrome where it is the system's: on macOS, the app menu and
//! what a double click on the title bar does.
//!
//! Linux and Windows have neither: the window is undecorated and the
//! interface draws its own title bar, buttons and menu. A Mac keeps its own
//! title bar under the interface's (src-tauri/tauri.macos.conf.json), and
//! with it two things a Mac app is expected to have:
//!
//! - **The menu bar.** zapper › About, Settings… (⌘,), Hide, Quit (⌘Q); Edit
//!   with Undo, Redo, Cut, Copy, Paste and Select All, without which ⌘C, ⌘V
//!   and ⌘A do nothing in a text field, since AppKit routes those keys
//!   through the menu; View › Enter Full Screen; Window › Minimise and Zoom.
//!   Settings… and About are the interface's to open, so they are sent to it
//!   (`events::MENU`); Quit closes the window, the same close as its red
//!   button and as every other way out, which saves the session.
//! - **The title bar's double click**, which System Settings › Desktop & Dock
//!   decides (zoom, fill, minimise or nothing). The interface's bar catches
//!   the click, since it is on top; [`title_bar_double_click`] does what the
//!   setting says.

use tauri::AppHandle;

/// What a double click on the title bar should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoubleClick {
    /// Zoom, or on macOS 15 "Fill": the window to the screen and back.
    Zoom,
    Minimise,
    Nothing,
}

/// The action for macOS's `AppleActionOnDoubleClick` (`Maximize`, `Fill`,
/// `Minimize`, `None`), or, where that was never set, the older
/// `AppleMiniaturizeOnDoubleClick` switch. Unset altogether is Zoom, the
/// system's default.
pub fn double_click_action(action: Option<&str>, legacy_minimise: bool) -> DoubleClick {
    match action {
        Some("Minimize") => DoubleClick::Minimise,
        Some("None") => DoubleClick::Nothing,
        Some(_) => DoubleClick::Zoom,
        None if legacy_minimise => DoubleClick::Minimise,
        None => DoubleClick::Zoom,
    }
}

/// A double click on the interface's title bar. On macOS, what the system
/// setting says; elsewhere (not called there today) maximise or restore.
#[tauri::command]
pub fn title_bar_double_click(window: tauri::WebviewWindow) -> Result<(), String> {
    let action = system_double_click();
    let w = window.clone();
    window
        .run_on_main_thread(move || {
            let done = match action {
                DoubleClick::Nothing => Ok(()),
                DoubleClick::Minimise => w.minimize(),
                DoubleClick::Zoom => zoom(&w),
            };
            if let Err(e) = done {
                tracing::warn!("title bar double click: {e}");
            }
        })
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
fn system_double_click() -> DoubleClick {
    use objc2_foundation::{ns_string, NSUserDefaults};
    let defaults = NSUserDefaults::standardUserDefaults();
    let action = defaults
        .stringForKey(ns_string!("AppleActionOnDoubleClick"))
        .map(|s| s.to_string());
    let legacy = defaults.boolForKey(ns_string!("AppleMiniaturizeOnDoubleClick"));
    double_click_action(action.as_deref(), legacy)
}

#[cfg(not(target_os = "macos"))]
fn system_double_click() -> DoubleClick {
    DoubleClick::Zoom
}

/// AppKit's own zoom on macOS (`-[NSWindow zoom:]`), which toggles between
/// the window's size and the screen's, as the title bar's double click and
/// Window › Zoom do. Maximise or restore elsewhere.
#[cfg(target_os = "macos")]
fn zoom(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    let ptr = window.ns_window()?;
    // SAFETY: on the main thread (run_on_main_thread), and the pointer is the
    // live NSWindow of a window that has not been closed.
    if let Some(ns) = unsafe { ptr.cast::<objc2_app_kit::NSWindow>().as_ref() } {
        ns.zoom(None);
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn zoom(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    if window.is_maximized()? {
        window.unmaximize()
    } else {
        window.maximize()
    }
}

/// Sets the macOS app menu. Nothing elsewhere: there, the interface's own
/// menu is the only one.
pub fn install_menu(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    if let Err(e) = mac_menu(app) {
        tracing::warn!("no app menu: {e}");
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

#[cfg(target_os = "macos")]
fn mac_menu(app: &AppHandle) -> tauri::Result<()> {
    use crate::ipc::types::{events, MenuCommand};
    use tauri::menu::{MenuBuilder, MenuItem, SubmenuBuilder};
    use tauri::{Emitter, Manager};

    let about = MenuItem::with_id(app, "about", "About zapper", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?;
    let quit = MenuItem::with_id(app, "quit", "Quit zapper", true, Some("CmdOrCtrl+Q"))?;
    let zapper = SubmenuBuilder::new(app, "zapper")
        .item(&about)
        .separator()
        .item(&settings)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .item(&quit)
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let view = SubmenuBuilder::new(app, "View").fullscreen().build()?;
    let window = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize()
        .build()?;
    let menu = MenuBuilder::new(app)
        .items(&[&zapper, &edit, &view, &window])
        .build()?;
    app.set_menu(menu)?;

    app.on_menu_event(|app, event| {
        let command = match event.id().as_ref() {
            "settings" => MenuCommand::Settings,
            "about" => MenuCommand::About,
            "quit" => {
                if let Some(w) = app.get_webview_window("main") {
                    if let Err(e) = w.close() {
                        tracing::warn!("quit: {e}");
                    }
                }
                return;
            }
            _ => return,
        };
        if let Err(e) = app.emit(events::MENU, command) {
            tracing::warn!("menu: {e}");
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_setting_decides_the_double_click() {
        assert_eq!(
            double_click_action(Some("Maximize"), false),
            DoubleClick::Zoom
        );
        assert_eq!(double_click_action(Some("Fill"), false), DoubleClick::Zoom);
        assert_eq!(
            double_click_action(Some("Minimize"), false),
            DoubleClick::Minimise
        );
        assert_eq!(
            double_click_action(Some("None"), true),
            DoubleClick::Nothing
        );
    }

    #[test]
    fn unset_falls_back_to_the_older_switch_then_zoom() {
        assert_eq!(double_click_action(None, true), DoubleClick::Minimise);
        assert_eq!(double_click_action(None, false), DoubleClick::Zoom);
    }
}
