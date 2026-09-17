//! zapper — a fast Xtream live-TV client.
//!
//! Module boundaries are fixed here deliberately: `surface` owns everything
//! that knows GTK exists, `xtream` owns the wire protocol, `catalog` owns
//! persistence, and `ipc` is the only thing that talks to the frontend.
//! Nothing else may reach across those lines.

pub mod catalog;
pub mod ipc;
pub mod surface;
pub mod xtream;

pub fn run() {
    tracing_subscriber::fmt::init();
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            ipc::catalog_cmds::categories,
            ipc::catalog_cmds::channels,
            ipc::catalog_cmds::channels_by_ids,
            ipc::catalog_cmds::search,
            ipc::catalog_cmds::guide,
            ipc::catalog_cmds::sync_status,
            ipc::catalog_cmds::sync_now,
            ipc::catalog_cmds::settings,
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
            ipc::player_cmds::set_subtitle_style,
            ipc::player_cmds::apply_playback_settings,
        ])
        .setup(|app| {
            use tauri::Manager;
            let window = app
                .get_webview_window("main")
                .ok_or("no main window")?;
            // Video lives beneath the webview, not in it. Failing here is
            // fatal: a running app with no surface is worse than no app.
            let overlay = surface::overlay::install(&window)?;
            surface::mpv::Player::new(&overlay.gl_area)?;
            // Not app.manage(): the overlay holds Rc and GTK objects, so it is
            // !Send. It lives in a GTK-main-thread local instead.
            surface::set_installed(overlay);

            // Refresh the mirror in the background. Startup never waits on it:
            // reads answer from SQLite, and a week-old cache opens as fast as
            // a fresh one.
            // Settings must take effect at launch, not only when changed.
            // Infallible and non-blocking; already on the GTK main thread.
            ipc::player_cmds::apply_saved_settings(app.handle());

            catalog::sync_engine::spawn_periodic(app.handle().clone());

            // Development hooks. Nothing here runs in a normal launch: they
            // exist because the composited window cannot be screenshotted on
            // this desktop, so verification has to drive the real app and
            // reconstruct what it drew.
            dev_hooks(app.handle());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start zapper");
}

/// Auto-play and self-capture, both opt-in through the environment.
///
/// `ZAPPER_AUTOPLAY=<stream_id>` starts a channel once the window is up, and
/// `ZAPPER_CAPTURE=<path>` writes a reconstruction of the window a few seconds
/// later and exits. Together they let a screenshot of the real application -
/// real interface over real video - be taken with nobody watching.
fn dev_hooks(app: &tauri::AppHandle) {
    let Ok(_) = std::env::var("ZAPPER_AUTOPLAY").or_else(|_| std::env::var("ZAPPER_CAPTURE"))
    else {
        return;
    };

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
        gtk::glib::timeout_add_seconds_local_once(after as u32, move || {
            surface::capture::request(&path);
            // The surface only redraws when mpv has a new frame, which is
            // right for an idle GPU and wrong for a capture: with nothing
            // playing the render handler would never run and the grab would
            // never happen. Nudge it for a second.
            let mut nudges = 0;
            gtk::glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
                surface::with_overlay(|ov| {
                    gtk::prelude::GLAreaExt::queue_render(&ov.gl_area);
                });
                nudges += 1;
                if nudges < 20 {
                    gtk::glib::ControlFlow::Continue
                } else {
                    gtk::glib::ControlFlow::Break
                }
            });
            gtk::glib::timeout_add_seconds_local_once(2, move || {
                match surface::with_overlay(surface::capture::finish) {
                    Some(Ok(p)) => println!("CAPTURED {}", p.display()),
                    Some(Err(e)) => eprintln!("CAPTURE FAILED: {e}"),
                    None => eprintln!("CAPTURE FAILED: no overlay on this thread"),
                }
                let _ = std::io::Write::flush(&mut std::io::stdout());
                std::process::exit(0);
            });
        });
    }
}
