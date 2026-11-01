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

/// What playing a channel has shown about it, remembered across restarts
/// and resyncs (see the `channel_quality` table). Every measured field is
/// `None` until a play got far enough to measure it; a channel that has only
/// ever failed has a row with nothing measured.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityDto {
    /// mpv's `video-format`: `h264`, `hevc`, `mpeg2video`.
    pub codec: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
    /// Fields rather than frames. `None` when it could not be told.
    pub interlaced: Option<bool>,
    pub is_hdr: bool,
    /// mpv's `audio-codec-name`: `aac`, `ac3`, `eac3`, `mp2`.
    pub audio_codec: Option<String>,
    pub audio_channels: Option<i64>,
    /// Unix seconds of the last measurement: "last checked".
    pub measured_at: Option<i64>,
    pub ok_count: i64,
    pub fail_count: i64,
    /// Unix seconds of the last genuine failure to load, cleared by the next
    /// play that shows a picture. A channel with this set is drawn dimmed.
    pub failed_at: Option<i64>,
    /// Why it failed, in words for the list's tooltip.
    pub fail_reason: Option<String>,
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
    /// How to write `num`, when the integer is not it: a subchannel's `5.1`,
    /// stored as `5001` so it sorts. `None` for nearly every channel, whose
    /// number is shown as it is. See `m3u::parse::channel_number`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_label: Option<String>,
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

/// Where a programme found by search sits relative to now, which decides what
/// choosing it does: tune in, look ahead in the guide, or play the archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiringDto {
    Live,
    Upcoming,
    /// Finished, on a channel whose archive still reaches back to it.
    /// Finished programmes nobody can play are not search results.
    Catchup,
}

/// One airing of a programme, with the channel that carries it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgrammeHitDto {
    pub channel: ChannelDto,
    pub programme: ProgrammeDto,
    pub airing: AiringDto,
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
    /// The last time playback skipped ahead to live on this stream: by
    /// itself, after a tune-in's connect burst or after stalls had left it
    /// behind, or because the viewer asked for live and it moved a second or
    /// more. `None` until that happens, and again from the next open.
    ///
    /// Reported because an automatic skip is otherwise unexplained — the
    /// picture jumps and the programme is suddenly a few seconds further on
    /// — and the viewer should be told it was the app, and how far. One
    /// asked for is reported the same way, because "how far was I behind?"
    /// is the obvious question after pressing Live.
    pub live_jump: Option<LiveJumpDto>,
    /// A live channel is being started at live: its first frame is up,
    /// held still, while the server's connect burst arrives, so that the
    /// picture starts moving at live rather than twenty seconds in the past
    /// and then jumping. The state stays `Opening` meanwhile; nothing is
    /// playing yet. See `ipc::live_start::Join`.
    pub joining_live: bool,
    /// Why a stream that stopped by itself stopped, when the state is
    /// `Failed` because of it: taken over by another device, refused, the
    /// network. `None` otherwise. See `ipc::ending`.
    pub ending: Option<crate::ipc::ending::Ending>,
    /// Reconnecting by itself after the network failed. `None` otherwise —
    /// and always after the provider ended or refused the stream, which is
    /// never reconnected without a person asking. See `connections`.
    pub reconnect: Option<ReconnectDto>,
}

/// One automatic reconnect, in progress or waiting its turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconnectDto {
    /// Which one, from 1.
    pub attempt: u32,
    /// Of how many before it gives up.
    pub of: u32,
    /// Asking the account whether a connection is free, before connecting.
    pub checking: bool,
}

/// The account's connections, for Settings → Account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionsDto {
    /// Allowed: as the provider counts, or as "Simultaneous streams" says.
    pub max: Option<u32>,
    /// Open on every device, when the provider counts; `None` when nobody can.
    pub active: Option<u32>,
    /// Streams this app holds.
    pub ours: u32,
    /// The provider (or the tuner) counted.
    pub counted: bool,
    /// The account cannot be asked, so "Simultaneous streams" applies.
    pub declared: bool,
    /// When the account expires, Unix seconds, if the provider says.
    pub expires: Option<i64>,
}

/// One jump forward to live, automatic or asked for. See
/// [`PlayerStateDto::live_jump`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveJumpDto {
    /// How far forward it went, in seconds, to a tenth.
    pub seconds: f64,
    /// Counts up across the session, so two jumps of the same distance are
    /// still two jumps: the frontend announces a jump when this changes,
    /// not when the distance does.
    pub seq: u64,
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
            live_jump: None,
            joining_live: false,
            ending: None,
            reconnect: None,
        }
    }
}

/// One audio or subtitle track of the stream that is open, as mpv lists it.
///
/// Every descriptive field is optional: a broadcast multiplex often carries
/// a language and nothing else, and some carry not even that. Naming the
/// track for a person is the interface's job (`logic/tracks.ts`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackDto {
    /// mpv's id, which is numbered per kind: audio 1 and subtitle 1 are two
    /// different tracks. What `set_audio_track` and `set_subtitle_track` take.
    pub id: i64,
    pub title: Option<String>,
    /// As the stream declares it: usually ISO 639-2 (`eng`, `fre`), sometimes
    /// 639-1 (`en`).
    pub lang: Option<String>,
    /// mpv's codec name: `ac3`, `aac`, `dvb_subtitle`, `dvb_teletext`.
    pub codec: Option<String>,
    /// Audio only: how many channels, 6 being 5.1.
    pub channels: Option<i64>,
    pub default: bool,
    pub forced: bool,
    /// Subtitles for the deaf and hard of hearing.
    pub hearing_impaired: bool,
    /// Audio description.
    pub visual_impaired: bool,
    pub selected: bool,
}

/// How the picture is fitted to its slot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AspectMode {
    /// The stream's own shape.
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "16:9")]
    Wide,
    #[serde(rename = "4:3")]
    Standard,
    #[serde(rename = "2.35:1")]
    Scope,
    /// The stream's own shape, scaled until the slot is covered and the
    /// overhang cropped.
    #[serde(rename = "fill")]
    Fill,
    /// Pulled to the slot's shape, whatever that does to the picture.
    #[serde(rename = "stretch")]
    Stretch,
}

/// mpv's `deinterlace`: `auto` deinterlaces only frames flagged interlaced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeinterlaceMode {
    /// What [`crate::surface::mpv::Player::new`] sets, so the player starts
    /// here. Free on progressive video: mpv checks each frame's interlaced
    /// flag and builds no filter for a frame without it.
    #[default]
    Auto,
    On,
    Off,
}

/// What the picture's menu offers: the stream's tracks, and how it is drawn.
///
/// Pushed as `events::PLAYER_OPTIONS` whenever any of it changes — the track
/// lists with every new stream, or when a multiplex announces a track late —
/// and read once with `player_options`. Separate from `PlayerStateDto`,
/// which is pushed several times a second and has no need to carry a track
/// list that changes once per channel.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerOptionsDto {
    pub audio: Vec<TrackDto>,
    pub subtitles: Vec<TrackDto>,
    /// Kept for the session: mpv holds these across channel changes.
    pub aspect: AspectMode,
    pub deinterlace: DeinterlaceMode,
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
            outline_size: 2.0,
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
    /// Offer playback to the desktop's media controls: MPRIS on Linux (the
    /// play/pause, next and previous keys, GNOME's media panel), the System
    /// Media Transport Controls on Windows. Off by default, because the
    /// desktop gives those keys to one player at a time and plenty of people
    /// want their music player to keep them.
    pub media_controls: bool,
    /// How the interface writes times of day: `"auto"` follows the system's
    /// locale, `"12h"` is "8:30 pm", `"24h"` is "20:30". A string rather than
    /// an enum, like `language`: the backend never reads it, and a value a
    /// newer build adds must not make this one throw the whole blob away
    /// (the frontend reads anything it does not know as `"auto"`).
    pub time_format: String,
    /// What a live channel does about the server's connect burst, which
    /// starts it twenty or thirty seconds in the past.
    pub live_start: LiveStart,
    /// How many streams the account allows at once, for an account that
    /// cannot say (a playlist, a Tvheadend server). Reconnecting by itself
    /// after a network failure is only safe with one to spare. Default 1.
    pub simultaneous_streams: u32,
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
            media_controls: false,
            time_format: "auto".to_owned(),
            live_start: LiveStart::default(),
            simultaneous_streams: 1,
        }
    }
}

/// What a live channel does about the connect burst a server opens it
/// with — the last twenty or thirty seconds it holds, which mpv plays from
/// the oldest. See `ipc::live_start`.
///
/// Read from any string, an unknown one being the default: a choice a newer
/// build adds must not make this one throw the whole settings blob away.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "String")]
pub enum LiveStart {
    /// Play from where the stream starts and stay there: never skip ahead
    /// by itself. Live is a key (L) or a click away.
    Stay,
    /// Play at once, then skip ahead to live once the burst is in, saying
    /// so. A picture soonest, and one jump.
    #[default]
    CatchUp,
    /// Hold the first frame until the burst is in, then start at live: no
    /// jump, at the cost of a second or more of still picture.
    Wait,
}

impl From<String> for LiveStart {
    fn from(s: String) -> Self {
        match s.as_str() {
            "stay" => LiveStart::Stay,
            "wait" => LiveStart::Wait,
            _ => LiveStart::CatchUp,
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

/// One channel's watch history, as a settings backup carries it: the
/// activity row behind a favourite, with the channel's name so the file
/// says something to whoever opens it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRowDto {
    pub stream_id: i64,
    #[serde(default)]
    pub name: Option<String>,
    pub watch_seconds: i64,
    #[serde(default)]
    pub last_watched: Option<i64>,
    #[serde(default)]
    pub open_count: i64,
}

/// A backup file, opened for import. The account, when there is one, stays
/// in Rust: the interface is told what it is, never what it holds.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupFileDto {
    /// The file's name, for saying what is being imported.
    pub name: String,
    /// The file's contents, without its account.
    pub document: serde_json::Value,
    pub account: Option<BackupAccountDto>,
    /// Why the file's account cannot be imported, when it has one that this
    /// build cannot use.
    pub account_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupAccountDto {
    /// `xtream`, `m3u`, `hdhomerun` or `tvheadend`.
    pub kind: String,
    /// Where and who as, with any credentials redacted.
    pub description: String,
    /// Whether it carries a password or a login inside an address.
    pub has_secret: bool,
}

/// Event names. Emitted by Rust, subscribed by React.
pub mod events {
    pub const SYNC_STATUS: &str = "zapper://sync-status";
    pub const PLAYER_STATE: &str = "zapper://player-state";
    pub const PLAYER_OPTIONS: &str = "zapper://player-options";
    pub const UPDATE: &str = "zapper://update";
    /// A [`super::MediaKey`] pressed on the desktop's media controls.
    pub const MEDIA_KEY: &str = "zapper://media-key";
    /// A [`super::MenuCommand`] chosen in the macOS app menu.
    pub const MENU: &str = "zapper://menu";
    /// A [`super::ChannelQualityDto`]: what playing a channel just added to
    /// its record.
    pub const CHANNEL_QUALITY: &str = "zapper://channel-quality";
}

/// A channel's record after playing it added to it, so lists already on
/// screen can show it without fetching their page again.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelQualityDto {
    pub stream_id: i64,
    pub quality: Option<QualityDto>,
}

/// What the desktop's media controls should show: the channel as the title,
/// what is on it as the line beneath, and its logo. Sent by the frontend,
/// which is the side that knows the channel and the guide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSessionDto {
    pub title: String,
    pub subtitle: Option<String>,
    pub art_url: Option<String>,
    pub playback: MediaPlayback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MediaPlayback {
    Playing,
    Paused,
    Stopped,
}

/// An item chosen in the macOS app menu that the interface carries out.
/// Emitted as `events::MENU`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MenuCommand {
    Settings,
    About,
}

/// A media key, or its button on the desktop's controls. Emitted as
/// `events::MEDIA_KEY`. What each one does is the frontend's decision: next
/// and previous step through the list it is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MediaKey {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Stop,
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
    /// "xtream", "m3u", "hdhomerun" or "tvheadend": which set of fields
    /// is in use. Every set is reported, so switching kinds in the pane shows
    /// the others filled in as they were left.
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
    /// The HDHomeRun's address, as typed. A LAN address: nothing to hide.
    pub hdhomerun_device: String,
    /// XMLTV guides used in place of SiliconDust's, redacted like the
    /// playlist's guides. Empty means SiliconDust's.
    pub hdhomerun_epg_urls: Vec<String>,
    pub hdhomerun_epg_redacted: bool,
    /// The Tvheadend server and who to log in as. The password follows the
    /// Xtream password's rule: only whether one is on file.
    pub tvheadend_url: String,
    pub tvheadend_username: String,
    pub tvheadend_has_password: bool,
}

/// An account as typed into Settings → Account and not saved: what the
/// connection test checks, so a change can be tried before it replaces the
/// account in use.
///
/// Each kind carries exactly what its save command takes, with the same
/// rules: an empty password and a `None` list keep what is on file, because
/// the pane is never given those values to send back.
///
/// No `Debug`: it carries passwords, and the one way to print it by accident
/// is not to be able to.
#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum AccountCandidateDto {
    Xtream {
        host: String,
        username: String,
        password: String,
    },
    M3u {
        playlist: Option<String>,
        epg_urls: Option<Vec<String>>,
        user_agent: String,
    },
    Hdhomerun {
        device: String,
        epg_urls: Option<Vec<String>>,
    },
    Tvheadend {
        url: String,
        username: String,
        password: String,
    },
}
