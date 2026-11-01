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
    /// How much live video is kept for rewinding, in megabytes (10^6 bytes).
    ///
    /// Not minutes: a time would have to be converted to mpv's byte budget at
    /// one assumed bitrate, so "4 minutes" would hold nearly eleven on a
    /// 6.8 Mbps channel and under four on a 4K one. mpv's budget is bytes and
    /// nothing else, so the setting says what it is, and the settings pane
    /// shows what that buys at common bitrates instead of promising a time it
    /// cannot keep.
    pub rewind_mb: u32,
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
    /// Look for a new release on GitHub a minute after startup and once a
    /// day after that. On by default; off means only Check for updates asks.
    pub check_for_updates: bool,
}

impl Default for SettingsDto {
    fn default() -> Self {
        Self {
            rewind_mb: 512,
            hardware_decode: true,
            tone_map_hdr: true,
            guide_hours: 3,
            sync_interval_hours: 6,
            language: "auto".to_owned(),
            subtitle: SubtitleStyleDto::default(),
            check_for_updates: true,
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
    pub const UPDATE: &str = "zapper://update";
}

/// A release the update dialog can show.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReleaseDto {
    /// `0.3.0`, `0.4.0-rc.1`.
    pub version: String,
    /// The release's title, `zapper 0.3.0`.
    pub name: String,
    /// The release's notes, GitHub-flavoured Markdown, shown as plain text.
    pub notes: String,
    /// The release page, the one thing a build from source is offered.
    pub url: String,
    pub prerelease: bool,
    pub published_at: Option<String>,
    /// This install's package in the release. `None` for a build from
    /// source, or a release that lacks one: then only the page is offered.
    pub asset: Option<UpdateAssetDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAssetDto {
    pub name: String,
    pub size: u64,
}

/// Where the update stands. Every change is pushed as `events::UPDATE`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum UpdateStateDto {
    /// The build does not know which repository its releases come from
    /// (`updates::RELEASES_REPO` is still the placeholder). Nothing is ever
    /// fetched in this state.
    NotConfigured,
    /// Not checked yet this run.
    Idle,
    Checking,
    UpToDate,
    Available {
        release: UpdateReleaseDto,
    },
    Downloading {
        release: UpdateReleaseDto,
        received: u64,
        total: u64,
    },
    /// Downloaded and checked against the release's SHA256SUMS.
    Ready {
        release: UpdateReleaseDto,
    },
    Installing {
        release: UpdateReleaseDto,
    },
    /// Installed over the running copy; a restart starts the new one.
    Installed {
        release: UpdateReleaseDto,
    },
    /// A download or an install went wrong. A failed *check* is not a state:
    /// it leaves the previous one and is reported to whoever asked.
    Failed {
        message: String,
        release: Option<UpdateReleaseDto>,
    },
}

/// The updater as the interface sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatusDto {
    /// This build's version.
    pub current: String,
    /// How this copy was installed: `deb`, `rpm`, `nsis`, `msi`, `dmg`, or
    /// `none` for a build from source, which can only be pointed at the page.
    pub kind: String,
    /// Unix seconds of the last check that got an answer, this run.
    pub last_checked: Option<i64>,
    pub state: UpdateStateDto,
}

/// What Install did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum InstallOutcomeDto {
    /// Installed over the running version: offer a restart.
    Installed,
    /// A Windows installer started and zapper is quitting so it can replace
    /// the files.
    Quitting,
    /// zapper could not install it itself (no pkexec, the password prompt was
    /// cancelled, the package manager failed): the command to run in a
    /// terminal, why, and the package manager's output when it ran.
    Manual {
        command: String,
        reason: String,
        output: Option<String>,
    },
}

/// A scrollback preview: what the picture looked like at a point in the
/// buffer, for the scrubber to show under the cursor.
///
/// Carries the moment it was actually taken rather than the moment that was
/// asked for. They differ by up to a couple of seconds — the thumbnails are
/// sampled, not continuous — and the interface says which it is showing
/// instead of implying a precision that is not there.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailDto {
    /// A `data:image/png;base64,...` URL. Inlined rather than served: it is a
    /// few kilobytes, it is discarded as soon as the pointer moves on, and a
    /// URL would need something to serve it from.
    pub data_url: String,
    /// Seconds behind the live edge of the frame this actually is.
    pub behind_live: f64,
}

/// One reading for the diagnostics panel — "stats for nerds".
///
/// Deliberately not part of `PlayerStateDto`. That one is pushed to every
/// window on every change; this is a great deal more, is only wanted while
/// the panel is open, and is therefore polled.
///
/// **No stream URL and no credentials.** The panel exists to be
/// screenshotted into a bug report, and an Xtream URL carries the account's
/// username and password in its query string.
///
/// Everything is optional because mpv drops most of it between files and
/// half of it never exists for a raw MPEG-TS multiplex. Absent means "this
/// stream does not say", which the interface draws as a dash.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsDto {
    pub container: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    /// Which hardware decoder is actually in use, as opposed to requested.
    pub hwdec: Option<String>,
    pub pixel_format: Option<String>,
    /// Colour primaries and transfer. An HDR stream on an SDR screen is the
    /// washed-out picture these two explain.
    pub primaries: Option<String>,
    pub gamma: Option<String>,
    pub video_bitrate: Option<f64>,
    pub audio_bitrate: Option<f64>,
    /// What the renderer is actually achieving, against what the container
    /// claims. They differ when frames are being dropped.
    pub measured_fps: Option<f64>,
    pub container_fps: Option<f64>,
    pub audio_samplerate: Option<i64>,
    pub audio_channels: Option<i64>,
    /// Seconds of demuxed data ahead of the playhead: buffer health.
    pub cache_duration: Option<f64>,
    /// Bytes per second currently arriving.
    pub cache_speed: Option<f64>,
    pub demuxer_bytes: Option<i64>,
    /// True while playback is stopped waiting for the cache to refill. This
    /// is a stutter, caught in the act.
    pub buffering: Option<bool>,
    pub cache_buffering_percent: Option<f64>,
    pub dropped_frames: Option<i64>,
    pub decoder_dropped_frames: Option<i64>,
    /// Audio ahead of video, in seconds. Drifting from zero is the thing
    /// that makes a stream feel wrong without looking wrong.
    pub avsync: Option<f64>,
    /// Seconds behind the end of the seekable window, and scrollback depth.
    /// Repeated from the player state so the panel is one round trip.
    pub behind_live: f64,
    pub buffered_back: f64,
    /// The last minute of buffer, throughput, drops and starvation, oldest
    /// first. Collected on the player heartbeat rather than by whoever is
    /// asking, so the graphs are already drawn when the panel opens — see
    /// `crate::ipc::health`.
    pub history: Vec<crate::ipc::health::SampleDto>,
    /// Times playback stopped to wait for data since the stream started. A
    /// stall drops no frames, so this is the only number that counts the
    /// stutter a viewer notices most.
    pub stalls: i64,
}

/// The Xtream account, as much of it as the interface may see.
///
/// **No password.** It is write-only from the frontend's point of view: it
/// can be set and it can be replaced, but it is never sent back, so it
/// cannot leak through a devtools inspector, a screenshot of one, or a
/// serialised state dump.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDto {
    pub host: String,
    pub username: String,
    /// Whether a password is on file. Enough to draw the field as "set"
    /// without carrying the value.
    pub has_password: bool,
    /// "zapper" when an account is on file, "none" when there is not one
    /// yet.
    pub source: String,
    /// Where the account is kept, so the answer to "where is this?" is on
    /// screen rather than in a commit message.
    pub path: String,
    /// "xtream" or "m3u": which of the two sets of fields is in use. Both
    /// are reported, so switching kinds in the pane shows the other set
    /// filled in as it was left.
    pub kind: String,
    /// The playlist location, with any credentials in it redacted. A
    /// provider's playlist URL usually *is* the account
    /// (`get.php?username=…&password=…`), so it gets the password's rule.
    pub playlist: String,
    /// True when `playlist` had something taken out, and so cannot be sent
    /// back as it stands: the pane shows it as set and keeps it unless a
    /// new one is typed.
    pub playlist_redacted: bool,
    /// Guide locations, redacted the same way.
    pub epg_urls: Vec<String>,
    pub epg_redacted: bool,
    /// Empty for "the default".
    pub user_agent: String,
}
