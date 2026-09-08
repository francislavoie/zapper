//! Playback control.
//!
//! Many accounts allow only a single connection, so every path here
//! must tear down before it opens. Never hold two streams, even briefly.

use crate::ipc::types::PlayerStateDto;
use crate::surface::VideoRect;

#[tauri::command]
pub async fn play(_stream_id: i64) -> Result<(), String> {
    Err("not implemented".into())
}

/// Replays a past programme on an archive-enabled channel.
/// `start` is Unix seconds; `duration_minutes` bounds the request.
#[tauri::command]
pub async fn play_catchup(
    _stream_id: i64,
    _start: i64,
    _duration_minutes: u32,
) -> Result<(), String> {
    Err("not implemented".into())
}

#[tauri::command]
pub async fn stop() -> Result<(), String> {
    Err("not implemented".into())
}

#[tauri::command]
pub async fn toggle_pause() -> Result<(), String> {
    Err("not implemented".into())
}

/// Negative seeks back. Clamped to the buffered window.
#[tauri::command]
pub async fn seek_relative(_seconds: f64) -> Result<(), String> {
    Err("not implemented".into())
}

#[tauri::command]
pub async fn seek_live() -> Result<(), String> {
    Err("not implemented".into())
}

/// 0-200; above 100 is software boost via `volume-max`.
#[tauri::command]
pub async fn set_volume(_volume: f64) -> Result<(), String> {
    Err("not implemented".into())
}

#[tauri::command]
pub async fn set_muted(_muted: bool) -> Result<(), String> {
    Err("not implemented".into())
}

/// Widely shared Web Audio parameters: threshold -50, knee 40, ratio 12, attack 0,
/// release 0.25.
#[tauri::command]
pub async fn set_compressor(_enabled: bool) -> Result<(), String> {
    Err("not implemented".into())
}

/// Video is not a DOM element; the frontend sends where it should be drawn.
#[tauri::command]
pub async fn set_video_rect(_rect: VideoRect) -> Result<(), String> {
    Err("not implemented".into())
}

#[tauri::command]
pub async fn player_state() -> Result<PlayerStateDto, String> {
    Err("not implemented".into())
}
