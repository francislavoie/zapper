//! zapper — a fast Xtream live-TV client.
//!
//! Module boundaries are fixed here deliberately: `surface` owns everything
//! that knows which windowing toolkit exists (GTK on Linux, Win32 on
//! Windows), and keeps each one in its own `surface/<platform>` module
//! behind a boundary of plain functions; `xtream` owns the server's wire
//! protocol and `m3u` the playlist/XMLTV one, `source` chooses between the
//! two for the account in use, `catalog` owns persistence, `paths` says where
//! on disk each platform keeps it, `updates` owns zapper's own releases
//! (checking, downloading, installing), `media_keys` the desktop's media
//! controls (MPRIS, the Windows SMTC), and `ipc` is the only thing that
//! talks to the frontend. Nothing else may reach across those lines.

/// Names the process so the compositor can identify the window.
///
/// Only GLib calls, and that is a constraint rather than an oversight: this
/// runs before Tauri has created anything, because the `app_id` is sent when
/// the surface is created and nothing later can change it. GDK is not
/// initialised yet, so anything from `gdk` — `set_program_class`, which
/// looks like the obvious companion to this — panics outright with "GDK has
/// not been initialized".
///
/// `g_set_prgname` is what GTK reads for the Wayland `app_id`, and on X11
/// GDK derives the WM_CLASS from the same name, so one call covers both.
#[cfg(all(feature = "gui", target_os = "linux"))]
fn set_app_id() {
    gtk::glib::set_prgname(Some(APP_ID));
    gtk::glib::set_application_name("zapper");
}

#[cfg(all(feature = "gui", not(target_os = "linux")))]
fn set_app_id() {}

// The core: everything the recording service shares with the app.
pub mod catalog;
pub mod connections;
pub mod harness;
pub mod hdhomerun;
pub mod ipc;
pub mod localtime;
pub mod logfile;
pub mod m3u;
pub mod paths;
pub mod recording;
pub mod source;
#[cfg(test)]
pub(crate) mod test_http;
pub mod tvheadend;
pub mod xtream;

// The desktop app alone (the `gui` feature, on by default).
#[cfg(feature = "gui")]
pub mod chrome;
#[cfg(feature = "gui")]
pub mod media_keys;
#[cfg(feature = "gui")]
pub mod surface;
#[cfg(feature = "gui")]
pub mod updates;
#[cfg(feature = "gui")]
pub mod window_state;

/// The name the desktop matches this window to.
///
/// On Wayland a window has no icon of its own: the compositor pairs the
/// toplevel's `app_id` with an installed `.desktop` file and takes the icon
/// from there. Setting a GTK window icon, which is what most advice
/// suggests, is an X11 mechanism and is simply ignored — the app shows the
/// generic placeholder however many PNGs are bundled.
///
/// So the `app_id` has to be a known, stable string rather than whatever
/// GTK infers from argv, and `tools/install-desktop.sh` installs
/// `zapper.desktop` under that exact name.
#[cfg(all(feature = "gui", target_os = "linux"))]
const APP_ID: &str = "zapper";

#[cfg(feature = "gui")]
pub fn run() {
    // An unattended run — a harness, an autoplay, a capture, a muted check —
    // gets a scratch account and catalogue, not the ones on this machine:
    // opening the app as a person's account syncs against their provider and
    // can spend the one connection they are watching on. Before anything
    // reads a path, and before Tauri starts any thread.
    if unattended() {
        harness::isolate("app");
    }
    // `zapper.log` in the data folder (the scratch one, for an unattended
    // run), and standard error for a terminal that may be watching.
    logfile::init("zapper", true);
    // Before any window exists: the app_id is sent when the surface is
    // created, and Tauri creates it before the setup hook runs.
    set_app_id();
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            ipc::catalog_cmds::categories,
            ipc::catalog_cmds::channels,
            ipc::catalog_cmds::channels_by_ids,
            ipc::catalog_cmds::channel_numbers,
            ipc::catalog_cmds::search,
            ipc::catalog_cmds::search_programmes,
            ipc::catalog_cmds::channel_index,
            ipc::catalog_cmds::guide,
            ipc::catalog_cmds::sync_status,
            ipc::catalog_cmds::sync_now,
            ipc::catalog_cmds::settings,
            ipc::catalog_cmds::account,
            ipc::catalog_cmds::set_account,
            ipc::catalog_cmds::set_m3u_account,
            ipc::catalog_cmds::set_hdhomerun_account,
            ipc::catalog_cmds::set_tvheadend_account,
            ipc::catalog_cmds::test_account,
            ipc::catalog_cmds::test_account_candidate,
            ipc::catalog_cmds::set_settings,
            ipc::catalog_cmds::guide_rows,
            ipc::catalog_cmds::favourites,
            ipc::catalog_cmds::forget_activity,
            ipc::player_cmds::play,
            ipc::player_cmds::play_catchup,
            ipc::player_cmds::stop,
            ipc::player_cmds::toggle_pause,
            ipc::player_cmds::seek_relative,
            ipc::player_cmds::thumbnail_at,
            ipc::player_cmds::seek_live,
            ipc::player_cmds::set_volume,
            ipc::player_cmds::set_muted,
            ipc::player_cmds::set_compressor,
            ipc::player_cmds::set_video_rect,
            ipc::player_cmds::player_state,
            ipc::player_cmds::player_stats,
            ipc::player_cmds::player_options,
            ipc::player_cmds::set_audio_track,
            ipc::player_cmds::set_subtitle_track,
            ipc::player_cmds::set_aspect,
            ipc::player_cmds::set_deinterlace,
            ipc::player_cmds::cancel_reconnect,
            ipc::player_cmds::connection_status,
            ipc::player_cmds::set_subtitle_style,
            ipc::player_cmds::apply_playback_settings,
            ipc::player_cmds::set_media_session,
            ipc::update_cmds::update_status,
            ipc::update_cmds::update_check,
            ipc::update_cmds::update_download,
            ipc::update_cmds::update_cancel,
            ipc::update_cmds::update_install,
            ipc::update_cmds::update_restart,
            ipc::update_cmds::update_open_page,
            ipc::backup_cmds::activity_rows,
            ipc::backup_cmds::import_activity,
            ipc::backup_cmds::export_backup,
            ipc::backup_cmds::open_backup,
            ipc::backup_cmds::import_backup_account,
            ipc::recording_cmds::recordings,
            ipc::recording_cmds::record_programme,
            ipc::recording_cmds::preview_recording,
            ipc::recording_cmds::recording_backups,
            ipc::recording_cmds::set_recording_backup,
            ipc::recording_cmds::cancel_recording,
            ipc::recording_cmds::delete_recording,
            ipc::recording_cmds::recording_status,
            ipc::recording_cmds::recording_settings,
            ipc::recording_cmds::set_recording_settings,
            ipc::recording_cmds::recording_network,
            ipc::recording_cmds::set_recording_network,
            ipc::recording_cmds::set_recording_password,
            ipc::recording_cmds::revoke_recording_device,
            ipc::recording_cmds::recording_service,
            ipc::recording_cmds::install_recording_service,
            ipc::recording_cmds::remove_recording_service,
            ipc::recording_cmds::set_recording_linger,
            ipc::recording_cmds::open_recording_folder,
            chrome::title_bar_double_click,
        ])
        // The save and open dialogs, for the backups. Used from Rust alone:
        // the capability grants the window no `dialog:` permission.
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            use tauri::Manager;
            let window = app
                .get_webview_window("main")
                .ok_or("no main window")?;
            // Before anything is drawn: the event loop has not run yet, so the
            // window opens at the remembered size rather than jumping to it.
            window_state::install(&window);
            // Video lives beneath the webview, not in it. Failing here is
            // fatal: a running app with no surface is worse than no app.
            surface::install(&window)?;

            // Refresh the mirror in the background. Startup never waits on it:
            // reads answer from SQLite, and a week-old cache opens as fast as
            // a fresh one.
            // Settings must take effect at launch, not only when changed.
            // Infallible and non-blocking; already on the main thread.
            ipc::player_cmds::apply_saved_settings(app.handle());

            catalog::sync_engine::spawn_periodic(app.handle().clone());
            // The recording service on this computer shares the account file
            // and the catalogue, and a browser signed in to it can change
            // either: the window hears of both from here.
            catalog::sync_engine::spawn_watch_elsewhere(
                app.handle().clone(),
                ipc::player_cmds::forget_source,
            );

            // Before the frontend can ask: its first `update_status` needs
            // the state to exist. The first check itself waits a minute.
            ipc::update_cmds::install(app.handle());

            // Registers with the desktop only if Settings › Playback says to;
            // off by default.
            media_keys::install(app.handle());

            // The recording service, back on after an update stopped it to
            // replace its program (Windows; a no-op elsewhere). Not from an
            // unattended run, whose scratch home is not the service's.
            if !unattended() {
                std::thread::spawn(recording::service::resume);
            }

            chrome::install_menu(app.handle());

            // Development hooks. Nothing here runs in a normal launch: they
            // exist because the composited window cannot be screenshotted on
            // this desktop, so verification has to drive the real app and
            // reconstruct what it drew.
            dev_hooks(app.handle());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to start zapper")
        .run(quit::on_event);
}

/// Quitting in order.
///
/// Left to itself, Tauri ends the process in the same turn of the event loop
/// that destroys the window: mpv's render context is then freed by a
/// thread-local destructor after its GL context is gone, and WebKit's web
/// process is left to notice the app has vanished and exit by itself, which
/// on Mesa can crash in libEGL's exit handler. So the close and the exit are
/// both held back until [`surface::shut_down`] has taken playback, the page
/// and the web process down, and then let through.
#[cfg(feature = "gui")]
mod quit {
    use std::sync::atomic::{AtomicU8, Ordering};
    use tauri::{AppHandle, Manager, RunEvent, WindowEvent};

    const RUNNING: u8 = 0;
    const STOPPING: u8 = 1;
    const STOPPED: u8 = 2;
    static STATE: AtomicU8 = AtomicU8::new(RUNNING);

    /// What to do once everything is down: close the window (which then
    /// exits, as the last window), or exit with the code asked for.
    enum Then {
        Close,
        Exit(i32),
    }

    pub fn on_event(app: &AppHandle, event: RunEvent) {
        let stopped = STATE.load(Ordering::Acquire) == STOPPED;
        match event {
            RunEvent::WindowEvent {
                label,
                event: WindowEvent::CloseRequested { api, .. },
                ..
            } if label == "main" && !stopped => {
                api.prevent_close();
                begin(app, Then::Close);
            }
            RunEvent::ExitRequested { code, api, .. } if !stopped => {
                api.prevent_exit();
                begin(app, Then::Exit(code.unwrap_or(0)));
            }
            _ => {}
        }
    }

    fn begin(app: &AppHandle, then: Then) {
        if STATE
            .compare_exchange(RUNNING, STOPPING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        tracing::info!("quitting");
        let app = app.clone();
        crate::surface::shut_down(move || {
            STATE.store(STOPPED, Ordering::Release);
            match (then, app.get_webview_window("main")) {
                // `destroy`, not `close`: the close was already asked for.
                (Then::Close, Some(w)) => {
                    if let Err(e) = w.destroy() {
                        tracing::warn!("closing the window: {e}");
                        app.exit(0);
                    }
                }
                (Then::Close, None) => app.exit(0),
                (Then::Exit(code), _) => app.exit(code),
            }
        });
    }
}

/// The variables that make this a run nobody is sitting in front of.
#[cfg(feature = "gui")]
const UNATTENDED: [&str; 4] = ["ZAPPER_AUTOPLAY", "ZAPPER_CAPTURE", "ZAPPER_MUTE", "ZAPPER_HARNESS"];

#[cfg(feature = "gui")]
fn unattended() -> bool {
    UNATTENDED
        .into_iter()
        .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()))
}

/// Auto-play, self-capture and silence, all opt-in through the environment.
///
/// `ZAPPER_AUTOPLAY=<stream_id>` starts a channel once the window is up, and
/// `ZAPPER_CAPTURE=<path>` writes a reconstruction of the window a few seconds
/// later and exits. Together they let a screenshot of the real application -
/// real interface over real video - be taken with nobody watching.
/// `ZAPPER_MUTE=1` only silences, for looking at the picture on a machine
/// whose speakers must stay quiet; it is also implied by the other two.
///
/// All of them, like `ZAPPER_HARNESS`, run on a scratch account (see
/// [`run`]), so an autoplay plays a fixture (`ZAPPER_FIXTURE_BASE`) or
/// nothing — never a channel through the account on this machine, unless
/// `ZAPPER_ALLOW_REAL_ACCOUNT=1` says so.
#[cfg(feature = "gui")]
fn dev_hooks(app: &tauri::AppHandle) {
    // `ZAPPER_QUIT_AFTER=<seconds>` closes the window the way its close
    // button does, so a harness can check that quitting is clean. Only for an
    // unattended run.
    if unattended() {
        if let Some(secs) = std::env::var("ZAPPER_QUIT_AFTER")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
        {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs_f64(secs.max(0.0))).await;
                use tauri::Manager;
                if let Some(w) = handle.get_webview_window("main") {
                    tracing::info!("closing the window for ZAPPER_QUIT_AFTER");
                    if let Err(e) = w.close() {
                        tracing::warn!("ZAPPER_QUIT_AFTER: {e}");
                    }
                }
            });
        }
    }

    let wanted = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    if !["ZAPPER_AUTOPLAY", "ZAPPER_CAPTURE", "ZAPPER_MUTE"]
        .into_iter()
        .any(wanted)
    {
        return;
    }

    // Anything driven by these hooks runs unattended, so kill the audio path
    // outright before a stream can open. `ao=null` cannot be undone by a later
    // volume or mute change, which muting can.
    if let Some(Err(e)) = surface::mpv::with_player(|p| p.set_property("ao", "null")) {
        tracing::warn!("could not silence playback for an unattended run: {e}");
    }

    if let Ok(id) = std::env::var("ZAPPER_AUTOPLAY") {
        if let Ok(stream_id) = id.parse::<i64>() {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                // After the frontend has mounted and asked for its first page.
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                if let Err(e) = ipc::player_cmds::play(handle, stream_id).await {
                    tracing::error!("autoplay failed: {e}");
                }
            });
        }
    }

    if let Ok(path) = std::env::var("ZAPPER_CAPTURE") {
        let after: u64 = std::env::var("ZAPPER_CAPTURE_AFTER")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(14);
        surface::request_capture(path.into(), std::time::Duration::from_secs(after));
    }
}
