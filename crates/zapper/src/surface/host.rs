//! A player in a window mpv makes itself: no Tauri, no webview, no toolkit.
//!
//! The platform surfaces put the picture under a webview (`linux`, `macos`,
//! `windows`). The host has no webview to go under. mpv opens its own window
//! (`force-window`) and presents into it with its own renderer (`gpu-next`),
//! exactly as the mpv program does, and everything else is the same
//! [`mpv::Player`], installed as the process's [`PlayerBackend`]. The player
//! checks run on it, inside a headless desktop.
//!
//! Scrollback thumbnails come as they do on Windows, where there is no
//! framebuffer of ours to copy either: mpv's `screenshot-raw`, shrunk on the
//! CPU ([`mpv::screenshot_thumbnail`]), on the host's own thread, at the
//! trickplay clock's pace (one every [`trickplay`]'s four seconds, 160x90).
//!
//! Linux only in practice: on macOS a libmpv window needs the Cocoa main
//! thread, which this does not give it.

use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

use crate::player::PlayerBackend;
use crate::surface::{mpv, trickplay};

/// What the host's window is called, and whether it may make a sound.
#[derive(Debug, Clone)]
pub struct HostOptions {
    /// The window's title.
    pub title: String,
    /// No audio output at all (`ao=null`). Also implied by `ZAPPER_MUTE`.
    pub muted: bool,
}

/// How often the thumbnail clock is looked at. The clock itself decides when
/// one is due; this only bounds how late that can be noticed. As on Windows.
const THUMB_POLL: Duration = Duration::from_secs(1);

/// Creates the player in a window of its own, installs it
/// (`crate::player::install`) and starts its thumbnails. Once per process:
/// there is one mpv.
pub fn open_host(opts: HostOptions) -> Result<Arc<dyn PlayerBackend>, String> {
    let muted = opts.muted || mute_asked(std::env::var_os("ZAPPER_MUTE"));
    let options = window_options(&opts.title, muted);
    let player = mpv::Player::new(|init| {
        for (key, value) in &options {
            init.set_property(key, value.clone())?;
        }
        Ok(())
    })
    .map_err(|e| format!("{e:#}"))?;
    // Before it is installed: nothing can ask it to load before the gate is
    // open.
    mpv::set_own_window();
    let player: Arc<dyn PlayerBackend> = Arc::new(player);
    crate::player::install(player.clone());

    std::thread::Builder::new()
        .name("zapper-host-thumbs".into())
        .spawn(thumbnails)
        .map_err(|e| format!("starting the thumbnail thread: {e}"))?;
    tracing::info!("video surface: mpv's own window");
    Ok(player)
}

/// mpv's options for presenting in a window of its own, after the shared
/// ones (`mpv::Player::new`).
///
/// - `vo=gpu-next`, as on Windows: mpv's own renderer, here on its Wayland
///   (or X11) EGL context, whichever the desktop offers.
/// - `force-window=yes`: the window from the start, not only once a file
///   with video opens, so a load has somewhere to go and the desktop sees
///   one window for the life of the player.
/// - mpv's own keyboard and mouse bindings off, as on Windows: whatever
///   drives the player owns its controls, and a stray key reaching mpv's
///   window must not pause or seek behind its back.
/// - `ao=null` when muted: nothing later (a volume or mute change) can undo
///   it, as for the app's unattended runs.
///
/// `keep-open` is left at mpv's default, as everywhere else: the controller
/// reads the end of a title from `end-file`.
fn window_options(title: &str, muted: bool) -> Vec<(&'static str, String)> {
    let mut o = vec![
        ("vo", "gpu-next".to_owned()),
        ("force-window", "yes".to_owned()),
        ("title", title.to_owned()),
        ("input-default-bindings", "no".to_owned()),
        ("input-vo-keyboard", "no".to_owned()),
    ];
    if muted {
        o.push(("ao", "null".to_owned()));
    }
    o
}

/// `ZAPPER_MUTE` set to anything but the empty string.
fn mute_asked(var: Option<OsString>) -> bool {
    var.is_some_and(|v| !v.is_empty())
}

/// Takes a thumbnail whenever one is due, until the player is uninstalled.
fn thumbnails() {
    loop {
        std::thread::sleep(THUMB_POLL);
        let Some(player) = crate::player::backend() else {
            return;
        };
        // Nothing loaded, nothing to photograph. Read before the clock, so
        // that an idle player does not use up the due time.
        let Some(at) = player.get_f64("playback-time") else {
            continue;
        };
        if !trickplay::is_due() {
            continue;
        }
        // Dropped if the stream changes while mpv takes it.
        let generation = trickplay::generation();
        if let Some(rgba) = mpv::screenshot_thumbnail() {
            trickplay::record_for(generation, at, rgba);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(o: &'a [(&'static str, String)], k: &str) -> Option<&'a str> {
        o.iter().find(|(key, _)| *key == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn the_host_window_is_mpvs_own() {
        let o = window_options("zapper", false);
        assert_eq!(get(&o, "vo"), Some("gpu-next"));
        assert_eq!(get(&o, "force-window"), Some("yes"));
        assert_eq!(get(&o, "title"), Some("zapper"));
        assert_eq!(get(&o, "input-default-bindings"), Some("no"));
        assert_eq!(get(&o, "ao"), None);
    }

    #[test]
    fn a_muted_host_has_no_audio_output() {
        assert_eq!(get(&window_options("t", true), "ao"), Some("null"));
        assert!(mute_asked(Some("1".into())));
        assert!(!mute_asked(Some("".into())));
        assert!(!mute_asked(None));
    }
}
