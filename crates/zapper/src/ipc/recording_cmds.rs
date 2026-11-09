//! Recording, from the app: the recording service is asked over its HTTP API
//! (`recording::client`), always from here and never from the webview, so
//! the API token stays out of the page. Installing and removing the service
//! (`recording::service`) runs `systemctl`, on a blocking thread.

use crate::recording::api::{
    ConflictDto, NetworkDto, NetworkSettingsDto, RecordingDto, RecordingSettingsDto,
    ScheduleRequest, StatusDto,
};
use crate::recording::backup::BackupsDto;
use crate::recording::client::Client;
use crate::recording::service::{self, ServiceStatusDto};

fn client() -> Result<Client, String> {
    Client::local().map_err(|e| format!("{e:#}"))
}

fn words(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command]
pub async fn recordings() -> Result<Vec<RecordingDto>, String> {
    let list = client()?.list().await.map_err(words)?;
    crate::ipc::handover::noticed(&list);
    Ok(list)
}

/// Schedules a programme. One on now on the channel the player shows keeps
/// what the player has of it, and on an account with no connection to spare
/// the player moves onto the service's connection (`ipc::handover`).
#[tauri::command]
pub async fn record_programme(request: ScheduleRequest) -> Result<RecordingDto, String> {
    crate::ipc::handover::record(request).await
}

/// The conflicts recording a programme would bring. A service too old to
/// say has none to tell.
#[tauri::command]
pub async fn preview_recording(request: ScheduleRequest) -> Result<Vec<ConflictDto>, String> {
    Ok(client()?.preview(&request).await.unwrap_or_default())
}

/// The channels a recording could fall back to, best first.
#[tauri::command]
pub async fn recording_backups(id: i64) -> Result<BackupsDto, String> {
    client()?.backups(id).await.map_err(words)
}

/// Chooses a recording's backup feed, or none.
#[tauri::command]
pub async fn set_recording_backup(id: i64, stream_id: Option<i64>) -> Result<RecordingDto, String> {
    client()?.set_backup(id, stream_id).await.map_err(words)
}

#[tauri::command]
pub async fn cancel_recording(id: i64) -> Result<(), String> {
    client()?.cancel(id).await.map_err(words)
}

#[tauri::command]
pub async fn delete_recording(id: i64, files: bool) -> Result<(), String> {
    client()?.delete(id, files).await.map(|_| ()).map_err(words)
}

#[tauri::command]
pub async fn recording_status() -> Result<StatusDto, String> {
    client()?.status().await.map_err(words)
}

#[tauri::command]
pub async fn recording_settings() -> Result<RecordingSettingsDto, String> {
    client()?.settings().await.map_err(words)
}

#[tauri::command]
pub async fn set_recording_settings(
    settings: RecordingSettingsDto,
) -> Result<RecordingSettingsDto, String> {
    // A folder the boot-time Windows service, running as an account of its
    // own, may not write yet: granted first, as this person.
    let dir = std::path::PathBuf::from(settings.output_dir.trim());
    if let Ok(Err(e)) = tokio::task::spawn_blocking(move || service::allow_folder(&dir)).await {
        tracing::warn!("recording service: the recordings folder: {e:#}");
    }
    client()?.set_settings(&settings).await.map_err(words)
}

#[tauri::command]
pub async fn recording_network() -> Result<NetworkDto, String> {
    client()?.network().await.map_err(words)
}

#[tauri::command]
pub async fn set_recording_network(settings: NetworkSettingsDto) -> Result<NetworkDto, String> {
    client()?.set_network(&settings).await.map_err(words)
}

/// The password other devices sign in with: passed through to the service,
/// which keeps only its hash.
#[tauri::command]
pub async fn set_recording_password(password: String) -> Result<NetworkDto, String> {
    client()?.set_password(&password).await.map_err(words)
}

#[tauri::command]
pub async fn revoke_recording_device(id: String) -> Result<NetworkDto, String> {
    client()?.revoke_device(&id).await.map_err(words)
}

/// The service on this computer: installed, running, answering, and with
/// which version.
#[tauri::command]
pub async fn recording_service() -> Result<ServiceStatusDto, String> {
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

#[tauri::command]
pub async fn install_recording_service(linger: bool) -> Result<ServiceStatusDto, String> {
    tokio::task::spawn_blocking(move || service::install(linger))
        .await
        .map_err(|e| e.to_string())?
        .map_err(words)?;
    // The service needs a moment to open its port; the status that follows
    // is the interface's next poll's to report.
    recording_service().await
}

#[tauri::command]
pub async fn remove_recording_service() -> Result<ServiceStatusDto, String> {
    tokio::task::spawn_blocking(service::remove)
        .await
        .map_err(|e| e.to_string())?
        .map_err(words)?;
    recording_service().await
}

#[tauri::command]
pub async fn set_recording_linger(on: bool) -> Result<ServiceStatusDto, String> {
    tokio::task::spawn_blocking(move || service::set_linger(on))
        .await
        .map_err(|e| e.to_string())?
        .map_err(words)?;
    recording_service().await
}

/// Opens a recording's folder in the file manager.
///
/// Only a folder the service named for one of its recordings: the page
/// passes the recording's id, never a path, so it cannot open anything else.
#[tauri::command]
pub async fn open_recording_folder(id: i64) -> Result<(), String> {
    let list = client()?.list().await.map_err(words)?;
    let rec = list
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| "that recording is gone".to_string())?;
    let folder = std::path::PathBuf::from(&rec.folder);
    let target = if folder.is_dir() {
        folder
    } else {
        folder
            .parent()
            .map(std::path::Path::to_path_buf)
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
