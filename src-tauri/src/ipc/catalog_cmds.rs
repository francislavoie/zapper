//! Catalogue reads and background sync.
//!
//! Every command here must answer from SQLite alone. Nothing on this path may
//! block on the network: startup latency is the whole point of the local
//! mirror, and a week-old cache must open exactly as fast as a fresh one.

use crate::ipc::types::{CategoryDto, ChannelDto, SyncStatusDto};

#[tauri::command]
pub async fn categories() -> Result<Vec<CategoryDto>, String> {
    Err("not implemented".into())
}

/// `category_id` of `None` means every channel, ordered by `num`.
#[tauri::command]
pub async fn channels(
    _category_id: Option<String>,
    _offset: u32,
    _limit: u32,
) -> Result<Vec<ChannelDto>, String> {
    Err("not implemented".into())
}

/// Fuzzy match over all channels. Separators are never returned.
#[tauri::command]
pub async fn search(_query: String, _limit: u32) -> Result<Vec<ChannelDto>, String> {
    Err("not implemented".into())
}

/// Programmes overlapping `[from, to]` for the given channels.
#[tauri::command]
pub async fn guide(
    _stream_ids: Vec<i64>,
    _from: i64,
    _to: i64,
) -> Result<Vec<(i64, Vec<crate::ipc::types::ProgrammeDto>)>, String> {
    Err("not implemented".into())
}

#[tauri::command]
pub async fn sync_status() -> Result<SyncStatusDto, String> {
    Err("not implemented".into())
}

/// Kicks a refresh. Returns immediately; progress arrives as events.
#[tauri::command]
pub async fn sync_now() -> Result<(), String> {
    Err("not implemented".into())
}
