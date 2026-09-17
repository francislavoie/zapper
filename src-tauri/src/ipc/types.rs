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
    /// Whether playback counts as live.
    ///
    /// Not `behind_live == 0`: the value is measured against the end of mpv's
    /// seekable window, which advances in chunks as the demuxer reads, so it
    /// sawtooths by a couple of seconds throughout perfectly normal live
    /// playback. Any tight threshold flickers. This carries hysteresis, and is
    /// the only thing the interface should use to decide.
    pub at_live_edge: bool,
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
            at_live_edge: true,
            volume: 100.0,
            muted: false,
            compressor: false,
            error: None,
        }
    }
}

/// How subtitles are drawn. Maps onto libass through mpv properties.
///
/// Modelled on Vimeo's controls rather than mpv's: people think in "box behind
/// the text" and "outline", not `sub-border-style`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubtitleBackdrop {
    /// A translucent box hugging the text. The default.
    Background,
    /// A full-width band behind the line.
    Window,
    /// Outline only.
    Outline,
    /// Drop shadow only.
    Shadow,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubtitleStyleDto {
    pub font: String,
    /// Points at 720p, scaled by mpv to the window.
    pub size: u32,
    /// `#rrggbb`.
    pub color: String,
    pub backdrop: SubtitleBackdrop,
    /// 0.0-1.0. Defaults to 0.25.
    pub backdrop_opacity: f64,
    pub outline_size: f64,
    pub shadow_offset: f64,
}

impl Default for SubtitleStyleDto {
    fn default() -> Self {
        Self {
            font: "sans-serif".into(),
            size: 38,
            color: "#ffffff".into(),
            backdrop: SubtitleBackdrop::Background,
            backdrop_opacity: 0.25,
            outline_size: 0.0,
            shadow_offset: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SettingsDto {
    /// How far back live playback can be rewound. Stored as minutes because
    /// that is the question a person actually asks; converted to the byte
    /// budget mpv wants.
    pub rewind_minutes: f64,
    pub hardware_decode: bool,
    /// Tone-map HDR down to SDR. Off would leave HDR washed out on an SDR
    /// panel, so it defaults on.
    pub tone_map_hdr: bool,
    /// Hours of programming the guide shows at once.
    pub guide_hours: u32,
    /// How often the background refresh runs.
    pub sync_interval_hours: u32,
    /// BCP-47 tag, or `"auto"` to follow the system.
    ///
    /// Only English ships today. The field exists so that adding a locale is
    /// dropping in a catalogue rather than retrofitting every string in the
    /// application, which is the expensive way to do this.
    pub language: String,
    pub subtitle: SubtitleStyleDto,
}

impl Default for SettingsDto {
    fn default() -> Self {
        Self {
            rewind_minutes: 4.0,
            hardware_decode: true,
            tone_map_hdr: true,
            guide_hours: 3,
            sync_interval_hours: 6,
            language: "auto".to_owned(),
            subtitle: SubtitleStyleDto::default(),
        }
    }
}

/// One channel's row in the guide.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuideRowDto {
    pub stream_id: i64,
    pub name: String,
    pub logo_url: Option<String>,
    pub has_archive: bool,
    pub archive_days: Option<u32>,
    pub programmes: Vec<ProgrammeDto>,
}

/// A channel the viewer keeps coming back to.
///
/// Earned rather than chosen: favourites are kept automatically from what
/// actually gets watched, so there is no "add to favourites" anywhere. The
/// score behind the ordering is weighted by recency, so a channel binged in
/// March does not outrank one watched all week.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavouriteDto {
    pub channel: ChannelDto,
    /// Total seconds watched, all time.
    pub watch_seconds: i64,
    /// Unix seconds.
    pub last_watched: Option<i64>,
    pub open_count: i64,
}

/// Event names. Emitted by Rust, subscribed by React.
pub mod events {
    pub const SYNC_STATUS: &str = "zapper://sync-status";
    pub const PLAYER_STATE: &str = "zapper://player-state";
}
