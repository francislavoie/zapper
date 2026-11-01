//! The recording service's HTTP contract (docs/recording.md).
//!
//! Shared by the service, which answers with these, and the app, which
//! forwards them to the frontend unchanged — so they are also mirrored in
//! `src/types.ts`. camelCase on the wire, like the Tauri DTOs.

use serde::{Deserialize, Serialize};

pub use super::naming::Naming;

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
    /// Where its files go: `<output folder>/<Show>` by default; the output
    /// folder itself for a flat layout.
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
    /// Where recordings go; `naming` lays them out inside.
    pub output_dir: String,
    /// How files are named and grouped in folders: a preset, or `custom`
    /// for `naming_pattern`. Omitted by an older client: the default.
    #[serde(default)]
    pub naming: Naming,
    /// The custom pattern (`{show}/{show} - {date} {time}`; see
    /// docs/recording.md). Kept while a preset is chosen, so going back to
    /// custom finds it again. Empty, or one `naming::validate` accepts.
    #[serde(default)]
    pub naming_pattern: String,
    /// Seconds before the programme's start and after its end.
    pub pad_before: i64,
    pub pad_after: i64,
    /// Add the wall-clock subtitle track to each file. It is never marked as
    /// a default track, so players leave it off until it is chosen.
    pub clock_track: bool,
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
    /// Unix seconds when it stops: the programme's end plus the padding.
    #[serde(default)]
    pub until: i64,
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
    /// Nothing is recording and nothing starts within five minutes
    /// (schedule::busy): the service can be stopped or restarted without
    /// spoiling anything. A service too old to say leaves it true; `active`
    /// still tells.
    #[serde(default = "yes")]
    pub idle: bool,
    /// When it next will be idle, if it is not now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_at: Option<i64>,
    /// The next padded start of a recording not yet begun.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_start: Option<i64>,
}

fn yes() -> bool {
    true
}

impl StatusDto {
    /// Stopping it now would cut a recording short (or one about to begin).
    pub fn busy(&self) -> bool {
        !self.idle || !self.active.is_empty()
    }
}

/// When `POST /api/shutdown` stops the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShutdownWhen {
    /// Carry on with the schedule, and stop at the first idle moment.
    Idle,
    /// Now: what is recording is closed and finished into a playable file
    /// first, and resumed (in a new part) if the service runs again while
    /// its programme is still on.
    Now,
}

/// `POST /api/shutdown`: the app's alone (this computer, its token).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShutdownRequest {
    pub when: ShutdownWhen,
    /// Start again once stopped: the same program, with the same arguments
    /// (so a replaced one runs its new version).
    #[serde(default)]
    pub restart: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionDto {
    pub version: String,
    pub api_version: u32,
}

/// `PUT /api/network`: whether other devices on the network may reach the
/// service, and how. The app's alone (this computer, its token).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkSettingsDto {
    /// Listen on every interface, not only 127.0.0.1. Needs a password set.
    pub enabled: bool,
    /// 1024–65535: the app's and the network's alike.
    pub port: u16,
    /// Other devices are served HTTPS; off means plain HTTP on the network.
    pub https: bool,
}

/// `GET /api/network`: the settings, and what follows from them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkDto {
    pub enabled: bool,
    pub port: u16,
    pub https: bool,
    /// A sign-in password is set (the password itself never leaves the
    /// service, and only its hash is kept).
    pub password_set: bool,
    /// The HTTPS certificate's SHA-256 fingerprint, `AB:CD:…`, once one was
    /// made: what a browser's certificate warning can be checked against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// Where another device reaches the service while network access is on:
    /// `https://192.168.1.20:8796`, one per address of this computer's
    /// network interfaces.
    pub addresses: Vec<String>,
    /// Why network access is not working as set, such as a port another
    /// program holds when the service started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The devices signed in, most recently used first.
    pub devices: Vec<DeviceDto>,
}

/// A device signed in with the password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDto {
    pub id: String,
    /// What it called itself, or what its browser says it is ("Firefox on
    /// Android").
    pub name: String,
    pub user_agent: String,
    /// Unix seconds.
    pub created: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<i64>,
    /// Where it was last seen from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

/// `PUT /api/network/password`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasswordRequest {
    pub password: String,
}

/// `POST /api/login`, from another device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub password: String,
    /// What to call the device in Settings; its browser's name otherwise.
    #[serde(default)]
    pub name: Option<String>,
}

/// `POST /api/login`'s answer: the device's token, sent as
/// `Authorization: Bearer <token>` from then on. Shown this once; the
/// service keeps only its hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginDto {
    pub token: String,
    pub device: DeviceDto,
}

/// Every error reply's body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDto {
    pub error: String,
}
