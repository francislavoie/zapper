//! The player's commands, as the window sends them: each one is
//! [`crate::player::api`]'s, under the same name and with the same
//! parameters. Only moving the video surface is the window's own, since the
//! toolkit's objects live on the main thread.

use std::time::Duration;

use tauri::AppHandle;

use crate::ipc::types::{
    AspectMode, ConnectionsDto, DeinterlaceMode, MediaSessionDto, PlayerOptionsDto,
    PlayerStateDto, SettingsDto, StatsDto, SubtitleStyleDto, ThumbnailDto, TitleThumbnailDto,
};
use crate::player::api;
use crate::surface::VideoRect;

/// Runs `f` on the main thread and waits for its result.
///
/// Blocking here is deliberate and bounded: the command is already on a worker
/// thread, and a hop that never answers must fail rather than wedge the
/// frontend forever. The toolkit's objects are `!Send` on every platform, so
/// only plain data crosses - never a toolkit handle.
async fn on_main<R: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce() -> Result<R, String> + Send + 'static,
) -> Result<R, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| format!("could not reach the main thread: {e}"))?;
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| "the main thread did not answer".to_owned())?
}

/// Video is not a DOM element; the frontend sends where it should be drawn.
#[tauri::command]
pub async fn set_video_rect(app: AppHandle, rect: VideoRect) -> Result<(), String> {
    on_main(&app, move || crate::surface::set_video_rect(rect)).await
}

#[tauri::command]
pub async fn cancel_reconnect() -> Result<(), String> {
    api::cancel_reconnect().await
}

#[tauri::command]
pub async fn connection_status() -> Result<ConnectionsDto, String> {
    api::connection_status().await
}

#[tauri::command]
pub async fn play(stream_id: i64) -> Result<(), String> {
    api::play(stream_id).await
}

#[tauri::command]
pub async fn play_movie(movie_id: i64, start: Option<f64>, held: Option<bool>) -> Result<(), String> {
    api::play_movie(movie_id, start, held).await
}

#[tauri::command]
pub async fn play_episode(episode_id: i64, start: Option<f64>, held: Option<bool>) -> Result<(), String> {
    api::play_episode(episode_id, start, held).await
}

#[tauri::command]
pub async fn play_next_episode(from_episode_id: i64, next_episode_id: i64) -> Result<(), String> {
    api::play_next_episode(from_episode_id, next_episode_id).await
}

#[tauri::command]
pub async fn play_catchup(stream_id: i64, start: i64, duration_minutes: u32) -> Result<(), String> {
    api::play_catchup(stream_id, start, duration_minutes).await
}

#[tauri::command]
pub async fn stop() -> Result<(), String> {
    api::stop().await
}

#[tauri::command]
pub async fn toggle_pause() -> Result<(), String> {
    api::toggle_pause().await
}

#[tauri::command]
pub async fn set_paused(paused: bool) -> Result<(), String> {
    api::set_paused(paused).await
}

#[tauri::command]
pub async fn seek_relative(seconds: f64) -> Result<(), String> {
    api::seek_relative(seconds).await
}

#[tauri::command]
pub async fn seek_to(seconds: f64) -> Result<(), String> {
    api::seek_to(seconds).await
}

#[tauri::command]
pub async fn seek_live() -> Result<(), String> {
    api::seek_live().await
}

#[tauri::command]
pub async fn set_volume(volume: f64) -> Result<(), String> {
    api::set_volume(volume).await
}

#[tauri::command]
pub async fn set_muted(muted: bool) -> Result<(), String> {
    api::set_muted(muted).await
}

#[tauri::command]
pub async fn set_compressor(enabled: bool) -> Result<(), String> {
    api::set_compressor(enabled).await
}

#[tauri::command]
pub async fn thumbnail_at(behind_live: f64) -> Result<Option<ThumbnailDto>, String> {
    api::thumbnail_at(behind_live).await
}

#[tauri::command]
pub async fn thumbnail_for_title(at: f64) -> Result<Option<TitleThumbnailDto>, String> {
    api::thumbnail_for_title(at).await
}

#[tauri::command]
pub async fn player_stats() -> Result<StatsDto, String> {
    api::player_stats().await
}

#[tauri::command]
pub async fn player_state() -> Result<PlayerStateDto, String> {
    api::player_state().await
}

#[tauri::command]
pub async fn player_options() -> Result<PlayerOptionsDto, String> {
    api::player_options().await
}

#[tauri::command]
pub async fn set_audio_track(id: i64) -> Result<PlayerOptionsDto, String> {
    api::set_audio_track(id).await
}

#[tauri::command]
pub async fn set_subtitle_track(id: Option<i64>) -> Result<PlayerOptionsDto, String> {
    api::set_subtitle_track(id).await
}

#[tauri::command]
pub async fn set_aspect(aspect: AspectMode) -> Result<PlayerOptionsDto, String> {
    api::set_aspect(aspect).await
}

#[tauri::command]
pub async fn set_deinterlace(mode: DeinterlaceMode) -> Result<PlayerOptionsDto, String> {
    api::set_deinterlace(mode).await
}

#[tauri::command]
pub async fn set_subtitle_style(style: SubtitleStyleDto) -> Result<(), String> {
    api::set_subtitle_style(style).await
}

#[tauri::command]
pub async fn apply_playback_settings(settings: SettingsDto) -> Result<(), String> {
    api::apply_playback_settings(settings).await
}

#[tauri::command]
pub async fn set_media_session(session: Option<MediaSessionDto>) -> Result<(), String> {
    api::set_media_session(session).await
}
