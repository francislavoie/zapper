//! The recording service on this computer, as the desktop app asks it: over
//! its HTTP API (`super::client`), always from Rust and never from a page,
//! so the API token stays out of it. What is more than one request is here;
//! Record on the channel playing is `player::handover`'s.

use std::path::{Path, PathBuf};

use super::api::RecordingSettingsDto;
use super::client::Client;
use super::service::{self, ServiceStatusDto};

fn client() -> Result<Client, String> {
    Client::local().map_err(|e| format!("{e:#}"))
}

/// The service on this computer: installed, running, answering, and with
/// which version.
pub async fn status() -> Result<ServiceStatusDto, String> {
    let mut s = tokio::task::spawn_blocking(service::status)
        .await
        .map_err(|e| e.to_string())?;
    if let Ok(c) = Client::local() {
        if let Ok(v) = c.version().await {
            s.version_mismatch = v.version != env!("CARGO_PKG_VERSION");
            s.version = Some(v.version);
        }
    }
    Ok(s)
}

/// Saves the service's settings. A folder the boot-time Windows service,
/// running as an account of its own, may not write yet is granted first, as
/// this person.
pub async fn set_settings(settings: RecordingSettingsDto) -> Result<RecordingSettingsDto, String> {
    let dir = PathBuf::from(settings.output_dir.trim());
    if let Ok(Err(e)) = tokio::task::spawn_blocking(move || service::allow_folder(&dir)).await {
        tracing::warn!("recording service: the recordings folder: {e:#}");
    }
    client()?
        .set_settings(&settings)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Opens a recording's folder in the file manager.
///
/// Only a folder the service named for one of its recordings: the caller
/// passes the recording's id, never a path, so it cannot open anything else.
pub async fn open_folder(id: i64) -> Result<(), String> {
    let list = client()?.list().await.map_err(|e| format!("{e:#}"))?;
    let rec = list
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| "that recording is gone".to_string())?;
    let folder = PathBuf::from(&rec.folder);
    let target = if folder.is_dir() {
        folder
    } else {
        folder
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "the recording's folder does not exist yet".to_string())?
    };
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = std::process::Command::new(opener)
        .arg(&target)
        .spawn()
        .map_err(|e| format!("could not open {}: {e}", target.display()))?;
    // Reaped, so it does not linger as a zombie for the app's lifetime.
    std::thread::spawn(move || child.wait());
    Ok(())
}
