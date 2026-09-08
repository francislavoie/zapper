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
            ipc::catalog_cmds::search,
            ipc::catalog_cmds::guide,
            ipc::catalog_cmds::sync_status,
            ipc::catalog_cmds::sync_now,
            ipc::player_cmds::play,
            ipc::player_cmds::play_catchup,
            ipc::player_cmds::stop,
            ipc::player_cmds::toggle_pause,
            ipc::player_cmds::seek_relative,
            ipc::player_cmds::seek_live,
            ipc::player_cmds::set_volume,
            ipc::player_cmds::set_muted,
            ipc::player_cmds::set_compressor,
            ipc::player_cmds::set_video_rect,
            ipc::player_cmds::player_state,
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
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start zapper");
}
