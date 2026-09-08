//! DTOs crossing the Tauri boundary.
//!
//! This file is the contract between the Rust backend and the React frontend
//! and is written once, up front, so that work on either side can proceed in
//! parallel without editing shared files. Mirrored exactly in `src/types.ts`.
//! Change both together or not at all.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryDto {
    pub id: String,
    pub name: String,
    /// Excludes separators and soft-deleted rows.
    pub channel_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityDto {
    pub codec: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
    pub is_hdr: bool,
    pub ok_count: i64,
    pub fail_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgrammeDto {
    /// Unix seconds.
    pub start: i64,
    pub stop: i64,
    pub title: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelDto {
    pub stream_id: i64,
    pub name: String,
    pub clean_name: String,
    pub num: Option<i64>,
    pub logo_url: Option<String>,
    pub category_id: Option<String>,
    pub has_archive: bool,
    pub archive_days: Option<u32>,
    /// Server-injected `###### NAME ######` divider. Render as a header; never
    /// playable, never a search hit, never counted as broken.
    pub is_separator: bool,
    /// Measured on first play. `None` means unknown, NOT bad - most of a
    /// large catalogue will never have been opened.
    pub quality: Option<QualityDto>,
    pub now_playing: Option<ProgrammeDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncPhase {
    Idle,
    Categories,
    Channels,
    Epg,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatusDto {
    pub phase: SyncPhase,
    /// Human-readable, safe to show in the UI. Never contains credentials.
    pub message: String,
    pub channels_total: i64,
    pub last_synced: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaybackState {
    Stopped,
    Opening,
    Playing,
    Paused,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerStateDto {
    pub state: PlaybackState,
    pub stream_id: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
    pub codec: Option<String>,
    /// Seconds of scrollback currently buffered.
    pub buffered_back: f64,
    /// Seconds behind the live edge; 0.0 when live.
    pub behind_live: f64,
    pub volume: f64,
    pub muted: bool,
    pub compressor: bool,
    pub error: Option<String>,
}

impl Default for PlayerStateDto {
    fn default() -> Self {
        Self {
            state: PlaybackState::Stopped,
            stream_id: None,
            width: None,
            height: None,
            fps: None,
            codec: None,
            buffered_back: 0.0,
            behind_live: 0.0,
            volume: 100.0,
            muted: false,
            compressor: false,
            error: None,
        }
    }
}

/// Event names. Emitted by Rust, subscribed by React.
pub mod events {
    pub const SYNC_STATUS: &str = "zapper://sync-status";
    pub const PLAYER_STATE: &str = "zapper://player-state";
}
