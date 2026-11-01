//! The desktop's media controls: MPRIS on Linux, the System Media Transport
//! Controls on Windows. Both through `souvlaki`.
//!
//! Opt-in (`SettingsDto::media_controls`). The desktop hands the play/pause,
//! next and previous keys to one player at a time, and someone who keeps a
//! music player running wants it to keep them; registering unasked would take
//! them away. So nothing here touches D-Bus or the SMTC until the setting is
//! on, and turning it off drops the registration at once, without a restart.
//!
//! One thread owns the controls. `souvlaki` hands back an object that is
//! created, updated and dropped by whoever holds it, and on Linux dropping it
//! joins a D-Bus thread that polls in one-second slices: no caller here should
//! wait on that, least of all the main thread or an IPC command. Commands are
//! posted to the owner and it applies them in order.
//!
//! What the keys *do* is not decided here. Next and previous step through the
//! list on screen, which only the frontend knows, so a key is emitted as
//! `events::MEDIA_KEY` and handled there, play/pause included, so every key
//! takes one path.

use std::sync::{mpsc, Mutex, OnceLock};

use tauri::{AppHandle, Emitter, Manager};

use crate::ipc::types::{events, MediaKey, MediaPlayback, MediaSessionDto};

enum Command {
    Enable,
    Disable,
    Session(Option<MediaSessionDto>),
}

static WORKER: OnceLock<Mutex<mpsc::Sender<Command>>> = OnceLock::new();

/// Starts the owning thread and applies the saved setting. Once, at setup.
pub fn install(app: &AppHandle) {
    let (tx, rx) = mpsc::channel();
    if WORKER.set(Mutex::new(tx)).is_err() {
        return;
    }
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("media-keys".into())
        .spawn(move || run(app, rx));
    if let Err(e) = spawned {
        tracing::warn!("media controls unavailable: {e}");
        return;
    }
    set_enabled(crate::catalog::settings::load_or_default().media_controls);
}

/// Registers with the desktop, or leaves it. Idempotent, so the settings
/// command can call it on every save. A no-op before [`install`], which is
/// what keeps unit tests of that command off the session bus.
pub fn set_enabled(on: bool) {
    send(if on { Command::Enable } else { Command::Disable });
}

/// What the controls should show. Kept while they are off, so turning them
/// on shows the channel already playing rather than nothing until the next
/// change.
pub fn set_session(session: Option<MediaSessionDto>) {
    send(Command::Session(session));
}

fn send(cmd: Command) {
    if let Some(tx) = WORKER.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(cmd);
        }
    }
}

fn run(app: AppHandle, rx: mpsc::Receiver<Command>) {
    let mut controls: Option<platform::Controls> = None;
    let mut session: Option<MediaSessionDto> = None;
    // What the desktop was last told, so a resend of the same thing — the
    // frontend sends on every change it sees, not every change that matters
    // here — is not a D-Bus signal.
    let mut shown: Option<Option<MediaSessionDto>> = None;

    for cmd in rx {
        match cmd {
            Command::Enable => {
                if controls.is_none() {
                    let attached = platform::window_handle(&app)
                        .and_then(|hwnd| platform::Controls::attach(hwnd, handler(app.clone())));
                    match attached {
                        Ok(c) => {
                            controls = Some(c);
                            shown = None;
                        }
                        // A desktop with no session bus is not a reason for
                        // the app to fail; the keys simply do nothing.
                        Err(e) => tracing::warn!("could not register media controls: {e}"),
                    }
                }
            }
            Command::Disable => {
                // Dropping detaches. Here, not on the caller's thread.
                controls = None;
                shown = None;
            }
            Command::Session(s) => session = s,
        }
        if let Some(c) = controls.as_mut() {
            if shown.as_ref() != Some(&session) {
                if let Err(e) = c.show(session.as_ref()) {
                    tracing::warn!("could not update media controls: {e}");
                }
                shown = Some(session.clone());
            }
        }
    }
}

/// What the desktop's button presses become.
fn handler(app: AppHandle) -> impl Fn(Pressed) + Send + 'static {
    move |pressed| match pressed {
        Pressed::Key(key) => {
            if let Err(e) = app.emit(events::MEDIA_KEY, key) {
                tracing::warn!("could not pass on a media key: {e}");
            }
        }
        // "Show the player" from the desktop's controls: the one request
        // handled here, because it is about the window rather than the list.
        Pressed::Raise => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }
    }
}

/// A press, as far as this app distinguishes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressed {
    Key(MediaKey),
    Raise,
}

/// What the desktop shows for a session: `(title, artist, cover)`.
///
/// The programme goes where a song's artist would, because that is the line
/// every media panel draws under the title; the album line is left empty
/// rather than repeating either. Artwork only from a URL the desktop can
/// fetch itself — a `data:` logo is fine in a webview and a broken image in
/// GNOME's panel.
pub fn metadata(s: &MediaSessionDto) -> (&str, Option<&str>, Option<&str>) {
    let subtitle = s
        .subtitle
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let cover = s.art_url.as_deref().map(str::trim).filter(|u| {
        let lower = u.to_ascii_lowercase();
        lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("file://")
    });
    (s.title.trim(), subtitle, cover)
}

/// The playback state to report. Nothing playing is `Stopped`, whatever the
/// last session said.
pub fn playback_of(s: Option<&MediaSessionDto>) -> MediaPlayback {
    s.map_or(MediaPlayback::Stopped, |s| s.playback)
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod platform {
    use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, PlatformConfig};
    use tauri::AppHandle;

    use super::{metadata, playback_of, Pressed};
    use crate::ipc::types::{MediaKey, MediaPlayback, MediaSessionDto};

    pub struct Controls(MediaControls);

    impl Controls {
        /// `hwnd` is the window the controls belong to on Windows, and
        /// ignored elsewhere.
        pub fn attach(
            hwnd: Option<*mut std::ffi::c_void>,
            on_press: impl Fn(Pressed) + Send + 'static,
        ) -> Result<Self, String> {
            let config = PlatformConfig {
                display_name: "zapper",
                // `org.mpris.MediaPlayer2.zapper` on the session bus.
                dbus_name: "zapper",
                hwnd,
            };
            let mut controls = MediaControls::new(config).map_err(|e| format!("{e:?}"))?;
            controls
                .attach(move |event| {
                    if let Some(p) = pressed(&event) {
                        on_press(p);
                    }
                })
                .map_err(|e| format!("{e:?}"))?;
            Ok(Self(controls))
        }

        pub fn show(&mut self, session: Option<&MediaSessionDto>) -> Result<(), String> {
            let meta = match session {
                Some(s) => {
                    let (title, artist, cover_url) = metadata(s);
                    MediaMetadata {
                        title: Some(title),
                        artist,
                        cover_url,
                        ..Default::default()
                    }
                }
                None => MediaMetadata::default(),
            };
            self.0.set_metadata(meta).map_err(|e| format!("{e:?}"))?;
            let playback = match playback_of(session) {
                MediaPlayback::Playing => souvlaki::MediaPlayback::Playing { progress: None },
                MediaPlayback::Paused => souvlaki::MediaPlayback::Paused { progress: None },
                MediaPlayback::Stopped => souvlaki::MediaPlayback::Stopped,
            };
            self.0.set_playback(playback).map_err(|e| format!("{e:?}"))
        }
    }

    #[cfg(target_os = "windows")]
    pub fn window_handle(app: &AppHandle) -> Result<Option<*mut std::ffi::c_void>, String> {
        use tauri::Manager;
        // The SMTC are per window: Windows asks for the one whose taskbar
        // entry and media overlay they belong to.
        let window = app.get_webview_window("main").ok_or("no main window")?;
        let hwnd = window.hwnd().map_err(|e| e.to_string())?;
        Ok(Some(hwnd.0))
    }

    #[cfg(not(target_os = "windows"))]
    pub fn window_handle(_: &AppHandle) -> Result<Option<*mut std::ffi::c_void>, String> {
        Ok(None)
    }

    /// The presses this app answers. Seeking, volume and the rest are left
    /// to the app's own controls: a live stream's position is not something
    /// a desktop slider can say anything sensible about.
    pub fn pressed(event: &MediaControlEvent) -> Option<Pressed> {
        Some(match event {
            MediaControlEvent::Play => Pressed::Key(MediaKey::Play),
            MediaControlEvent::Pause => Pressed::Key(MediaKey::Pause),
            MediaControlEvent::Toggle => Pressed::Key(MediaKey::Toggle),
            MediaControlEvent::Next => Pressed::Key(MediaKey::Next),
            MediaControlEvent::Previous => Pressed::Key(MediaKey::Previous),
            MediaControlEvent::Stop => Pressed::Key(MediaKey::Stop),
            MediaControlEvent::Raise => Pressed::Raise,
            _ => return None,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use souvlaki::SeekDirection;
        use std::time::Duration;

        #[test]
        fn the_transport_buttons_become_keys() {
            for (event, key) in [
                (MediaControlEvent::Play, MediaKey::Play),
                (MediaControlEvent::Pause, MediaKey::Pause),
                (MediaControlEvent::Toggle, MediaKey::Toggle),
                (MediaControlEvent::Next, MediaKey::Next),
                (MediaControlEvent::Previous, MediaKey::Previous),
                (MediaControlEvent::Stop, MediaKey::Stop),
            ] {
                assert_eq!(pressed(&event), Some(Pressed::Key(key)), "{event:?}");
            }
            assert_eq!(pressed(&MediaControlEvent::Raise), Some(Pressed::Raise));
        }

        #[test]
        fn seeking_volume_and_quit_are_left_alone() {
            for event in [
                MediaControlEvent::Seek(SeekDirection::Forward),
                MediaControlEvent::SeekBy(SeekDirection::Backward, Duration::from_secs(5)),
                MediaControlEvent::SetVolume(0.5),
                MediaControlEvent::OpenUri("http://example.com".into()),
                // A desktop widget's "quit" must not close the app.
                MediaControlEvent::Quit,
            ] {
                assert_eq!(pressed(&event), None, "{event:?}");
            }
        }

        /// The registration as a desktop sees it: the metadata it reads and
        /// the buttons it presses, over a real session bus.
        ///
        /// Ignored, and refuses to run without `ZAPPER_MPRIS_TEST=1`, because
        /// it must only ever see a private bus — on the desktop's own bus it
        /// would appear in the media panel beside whatever is playing. Run
        /// it as:
        ///
        /// ```text
        /// ZAPPER_MPRIS_TEST=1 dbus-run-session -- \
        ///     cargo test --lib media_keys -- --ignored
        /// ```
        #[cfg(target_os = "linux")]
        #[test]
        #[ignore = "needs a private session bus: see the doc comment"]
        fn a_desktop_reads_the_session_and_presses_the_buttons() {
            use dbus::arg::{prop_cast, PropMap};
            use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
            use std::sync::mpsc;
            use std::time::Instant;

            assert_eq!(
                std::env::var("ZAPPER_MPRIS_TEST").as_deref(),
                Ok("1"),
                "run under dbus-run-session with ZAPPER_MPRIS_TEST=1"
            );

            let (tx, rx) = mpsc::channel();
            let mut controls = Controls::attach(None, move |p| {
                let _ = tx.send(p);
            })
            .expect("attach");
            controls
                .show(Some(&MediaSessionDto {
                    title: "Northwind One HD".into(),
                    subtitle: Some("The News at Six".into()),
                    art_url: Some("https://img.example/northwind1.png".into()),
                    playback: MediaPlayback::Playing,
                }))
                .unwrap();

            let conn = dbus::blocking::Connection::new_session().unwrap();
            let player = conn.with_proxy(
                "org.mpris.MediaPlayer2.zapper",
                "/org/mpris/MediaPlayer2",
                Duration::from_secs(5),
            );
            const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

            // The update crosses souvlaki's own thread; wait for it to land.
            let deadline = Instant::now() + Duration::from_secs(10);
            let meta: PropMap = loop {
                let m: PropMap = player.get(PLAYER, "Metadata").unwrap();
                if prop_cast::<String>(&m, "xesam:title").is_some() || Instant::now() > deadline {
                    break m;
                }
                std::thread::sleep(Duration::from_millis(100));
            };
            assert_eq!(
                prop_cast::<String>(&meta, "xesam:title").map(String::as_str),
                Some("Northwind One HD")
            );
            assert_eq!(
                prop_cast::<Vec<String>>(&meta, "xesam:artist"),
                Some(&vec!["The News at Six".to_owned()])
            );
            assert_eq!(
                prop_cast::<String>(&meta, "mpris:artUrl").map(String::as_str),
                Some("https://img.example/northwind1.png")
            );
            let status: String = player.get(PLAYER, "PlaybackStatus").unwrap();
            assert_eq!(status, "Playing");
            let identity: String = player.get("org.mpris.MediaPlayer2", "Identity").unwrap();
            assert_eq!(identity, "zapper");

            for (method, want) in [
                ("PlayPause", Pressed::Key(MediaKey::Toggle)),
                ("Next", Pressed::Key(MediaKey::Next)),
                ("Previous", Pressed::Key(MediaKey::Previous)),
                ("Pause", Pressed::Key(MediaKey::Pause)),
            ] {
                let () = player.method_call(PLAYER, method, ()).unwrap();
                assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(want), "{method}");
            }

            // Turning the setting off leaves the bus.
            drop(controls);
            let names: (Vec<String>,) = conn
                .with_proxy("org.freedesktop.DBus", "/", Duration::from_secs(5))
                .method_call("org.freedesktop.DBus", "ListNames", ())
                .unwrap();
            assert!(!names.0.iter().any(|n| n == "org.mpris.MediaPlayer2.zapper"));
        }
    }
}

/// Everywhere else (macOS, so far): the setting is accepted and does nothing.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod platform {
    use super::Pressed;
    use crate::ipc::types::MediaSessionDto;
    use tauri::AppHandle;

    pub struct Controls;

    pub fn window_handle(_: &AppHandle) -> Result<Option<*mut std::ffi::c_void>, String> {
        Ok(None)
    }

    impl Controls {
        pub fn attach(
            _: Option<*mut std::ffi::c_void>,
            _: impl Fn(Pressed) + Send + 'static,
        ) -> Result<Self, String> {
            Err("media controls are not supported on this platform yet".into())
        }

        pub fn show(&mut self, _: Option<&MediaSessionDto>) -> Result<(), String> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(subtitle: Option<&str>, art: Option<&str>) -> MediaSessionDto {
        MediaSessionDto {
            title: " Northwind One HD ".into(),
            subtitle: subtitle.map(Into::into),
            art_url: art.map(Into::into),
            playback: MediaPlayback::Paused,
        }
    }

    #[test]
    fn the_channel_is_the_title_and_the_programme_the_line_beneath() {
        let s = session(Some("The News at Six"), Some("https://img.example/northwind1.png"));
        assert_eq!(
            metadata(&s),
            (
                "Northwind One HD",
                Some("The News at Six"),
                Some("https://img.example/northwind1.png")
            )
        );
    }

    #[test]
    fn a_blank_programme_is_no_programme() {
        assert_eq!(metadata(&session(Some("  "), None)).1, None);
        assert_eq!(metadata(&session(None, None)).1, None);
    }

    #[test]
    fn only_artwork_the_desktop_can_fetch_is_offered() {
        for ok in ["http://a/b.png", "HTTPS://a/b.png", "file:///tmp/logo.png"] {
            assert_eq!(metadata(&session(None, Some(ok))).2, Some(ok), "{ok}");
        }
        for bad in ["data:image/svg+xml;base64,AAAA", "", "logo.png", "javascript:x"] {
            assert_eq!(metadata(&session(None, Some(bad))).2, None, "{bad}");
        }
    }

    #[test]
    fn nothing_playing_reports_stopped() {
        assert_eq!(playback_of(None), MediaPlayback::Stopped);
        let s = session(None, None);
        assert_eq!(playback_of(Some(&s)), MediaPlayback::Paused);
    }

    #[test]
    fn keys_cross_the_boundary_in_camel_case() {
        // The frontend matches on these strings.
        assert_eq!(serde_json::to_string(&MediaKey::Previous).unwrap(), "\"previous\"");
        assert_eq!(serde_json::to_string(&MediaKey::Toggle).unwrap(), "\"toggle\"");
        let s: MediaSessionDto = serde_json::from_str(
            r#"{"title":"T","subtitle":null,"artUrl":"http://x","playback":"playing"}"#,
        )
        .unwrap();
        assert_eq!(s.playback, MediaPlayback::Playing);
        assert_eq!(s.art_url.as_deref(), Some("http://x"));
    }
}
