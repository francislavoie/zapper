//! zapper — a fast Xtream live-TV client.
//!
//! Module boundaries are fixed here deliberately: `surface` owns everything
//! that knows GTK exists, `xtream` owns the wire protocol, and `catalog` owns
//! persistence. Nothing else may reach across those lines.

pub mod catalog;
pub mod surface;
pub mod xtream;

pub fn run() {
    tracing_subscriber::fmt::init();
    tauri::Builder::default()
        .setup(|_app| Ok(()))
        .run(tauri::generate_context!())
        .expect("failed to start zapper");
}
