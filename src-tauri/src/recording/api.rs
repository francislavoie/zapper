//! The recording service's HTTP contract (docs/recording.md).
//!
//! Shared by the service, which answers with these, and the app, which
//! forwards them to the frontend unchanged — so they are also mirrored in
//! `src/types.ts`. camelCase on the wire, like the Tauri DTOs.

use serde::{Deserialize, Serialize};

/// Bumped when a request or a reply changes shape incompatibly.
pub const API_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordingState {
    /// Waiting for its start (less the padding).
    Scheduled,
    /// Capturing now.
    Recording,
    /// Finished, with at least one file.
    Done,
    /// Finished with nothing to show for it: never started (the service was
    /// not running), or no feed ever delivered.
    Failed,
}

impl RecordingState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Recording => "recording",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "scheduled" => Self::Scheduled,
            "recording" => Self::Recording,
            "done" => Self::Done,
            "failed" => Self::Failed,
            _ => return None,
        })
    }
}

/// One file a recording produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartDto {
    /// Absolute path. A `.ts` while capturing, the `.mkv` once finished (or
    /// the `.ts` kept as it is if it could not be remuxed).
    pub path: String,
    /// The channel this part came from: the main feed or the backup.
    pub stream_id: i64,
    pub bytes: u64,
    /// Unix seconds.
    pub started: i64,
    pub ended: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingDto {
    pub id: i64,
    pub stream_id: i64,
    /// The feed switched to when the main one fails.
    pub backup_stream_id: Option<i64>,
    pub channel_name: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
    /// The programme, Unix seconds.
    pub start: i64,
    pub stop: i64,
    /// Seconds recorded before `start` and after `stop`.
    pub pad_before: i64,
    pub pad_after: i64,
    pub state: RecordingState,
    pub parts: Vec<PartDto>,
    /// Things worth knowing about how it went: a switch to the backup feed,
    /// a refusal, a remux that failed and left the `.ts`.
    pub notes: Vec<String>,
    /// Why it failed, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Where its files go: `<output folder>/<Show>`.
    pub folder: String,
    /// While recording: why it is not capturing right now, such as
    /// "waiting for a free connection".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<String>,
}

/// `POST /api/recordings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleRequest {
    pub stream_id: i64,
    /// Omitted: the service picks another channel carrying the same guide
    /// channel, if there is one. Pass the main feed's own id for "none".
    #[serde(default)]
    pub backup_stream_id: Option<i64>,
    #[serde(default)]
    pub channel_name: Option<String>,
    pub title: String,
    #[serde(default)]
    pub episode_title: Option<String>,
    #[serde(default)]
    pub season: Option<u32>,
    #[serde(default)]
    pub episode: Option<u32>,
    pub start: i64,
    pub stop: i64,
    /// Omitted: the service's settings.
    #[serde(default)]
    pub pad_before: Option<i64>,
    #[serde(default)]
    pub pad_after: Option<i64>,
}

/// `GET`/`PUT /api/settings`: what the service does with every recording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingSettingsDto {
    /// Where recordings go; each show gets a folder inside.
    pub output_dir: String,
    /// Seconds before the programme's start and after its end.
    pub pad_before: i64,
    pub pad_after: i64,
    /// Add the wall-clock subtitle track to each file. It is never marked as
    /// a default track, so players leave it off until it is chosen.
    pub clock_track: bool,
    /// How many streams the account allows at once, for accounts that cannot
    /// be asked (playlists). Xtream accounts report their own.
    pub simultaneous_streams: u32,
    /// The folder used when none is chosen, so a client can offer to go back
    /// to it. Ignored when settings are written.
    #[serde(default)]
    pub default_output_dir: String,
}

/// A capture in progress, for `GET /api/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveDto {
    pub id: i64,
    pub title: String,
    /// The feed being captured now.
    pub stream_id: i64,
    pub bytes: u64,
    /// Unix seconds when the current part began.
    pub since: i64,
    /// Why it is not capturing right now, if it is not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusDto {
    pub version: String,
    pub api_version: u32,
    pub active: Vec<ActiveDto>,
    pub output_dir: String,
    /// Free and total bytes on the output folder's disk, when they could be
    /// read.
    pub disk_free: Option<u64>,
    pub disk_total: Option<u64>,
    /// Unix seconds, the service's clock: a wrong clock on a box records at
    /// the wrong times, so it is worth showing.
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionDto {
    pub version: String,
    pub api_version: u32,
}

/// Every error reply's body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDto {
    pub error: String,
}
