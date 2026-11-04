// Mirror of src-tauri/src/ipc/types.rs. Change both together or not at all.
// Rust serialises camelCase, so these names match the wire exactly.

export interface CategoryDto {
  id: string;
  name: string;
  /** Excludes separators and soft-deleted rows. */
  channelCount: number;
}

/** What playing a channel has shown about it, remembered across restarts
 *  and resyncs. Measured fields are null until a play got far enough to
 *  measure them; a channel that has only ever failed has nothing measured. */
export interface QualityDto {
  /** mpv's `video-format`: "h264", "hevc", "mpeg2video". */
  codec: string | null;
  width: number | null;
  height: number | null;
  fps: number | null;
  /** Fields rather than frames; null when it could not be told. */
  interlaced: boolean | null;
  isHdr: boolean;
  /** mpv's `audio-codec-name`: "aac", "ac3", "eac3", "mp2". */
  audioCodec: string | null;
  audioChannels: number | null;
  /** Unix seconds of the last measurement: "last checked". */
  measuredAt: number | null;
  okCount: number;
  failCount: number;
  /** Unix seconds of the last genuine failure to load, cleared by the next
   *  play that shows a picture. Such a channel is drawn dimmed. */
  failedAt: number | null;
  /** Why, in words. */
  failReason: string | null;
}

/** A channel's record after playing it added to it. */
export interface ChannelQualityDto {
  streamId: number;
  quality: QualityDto | null;
}

export interface ProgrammeDto {
  /** Unix seconds. */
  start: number;
  stop: number;
  title: string;
  description: string | null;
  /** One-based, when the guide gives a clean episode number; absent
   *  otherwise. */
  season?: number | null;
  episode?: number | null;
}

export interface ChannelDto {
  streamId: number;
  name: string;
  cleanName: string;
  num: number | null;
  /** How to write `num` when the integer is not it: an over-the-air
   *  subchannel's "5.1", which `num` holds as 5001 so lists sort by it.
   *  Absent for nearly every channel. Show `numLabel ?? num`. */
  numLabel?: string | null;
  logoUrl: string | null;
  categoryId: string | null;
  hasArchive: boolean;
  archiveDays: number | null;
  /** Panel-injected `###### NAME ######` divider: render as a header, never
   *  playable, never a search hit, never counted as broken. */
  isSeparator: boolean;
  /** Measured on first play. `null` means unknown, NOT bad — most of a
   *  large catalogue will never have been opened. */
  quality: QualityDto | null;
  nowPlaying: ProgrammeDto | null;
}

export type SyncPhase = "idle" | "categories" | "channels" | "epg" | "failed";

export interface SyncStatusDto {
  phase: SyncPhase;
  message: string;
  channelsTotal: number;
  lastSynced: number | null;
}

export type PlaybackState =
  | "stopped"
  | "opening"
  | "playing"
  | "paused"
  | "failed";

export interface PlayerStateDto {
  state: PlaybackState;
  streamId: number | null;
  width: number | null;
  height: number | null;
  fps: number | null;
  codec: string | null;
  /** Seconds of scrollback currently buffered. */
  bufferedBack: number;
  /** Seconds behind the live edge; 0 when live. */
  behindLive: number;
  /** Whether playback counts as live. Not `behindLive === 0`: that value
   *  sawtooths by a couple of seconds during normal playback as the demuxer
   *  reads, so a tight threshold flickers. This carries hysteresis and is the
   *  only thing the interface should use to decide. */
  atLiveEdge: boolean;
  volume: number;
  muted: boolean;
  compressor: boolean;
  error: string | null;
  /** The last time playback skipped ahead to live on this stream — by
   *  itself, after a tune-in's connect burst or after stalls, or because the
   *  viewer pressed Live and it moved a second or more — or null. Cleared by
   *  the next open. */
  liveJump: LiveJumpDto | null;
  /** A live channel is being started at live: its first frame is up and held
   *  still while the server's connect burst arrives, so the picture starts
   *  moving at live instead of twenty seconds back and then jumping. `state`
   *  stays "opening" meanwhile. */
  joiningLive: boolean;
  /** Why a stream that stopped by itself stopped, while `state` is
   *  "failed" because of it. `null` otherwise. See `ipc::ending`. */
  ending: StreamEnding | null;
  /** Reconnecting by itself after the network failed: which attempt, of how
   *  many, and whether it is asking the account for a free connection
   *  first. `null` otherwise. Only ever after a network failure, never after
   *  the provider ended or refused the stream. */
  reconnect: ReconnectDto | null;
  /** The movie that is open, with how far in it is; `null` for a live
   *  channel and when nothing is open. A title has no `streamId`. */
  onDemand: OnDemandDto | null;
}

/** Mirrors `ReconnectDto`. */
export interface ReconnectDto {
  attempt: number;
  of: number;
  checking: boolean;
}

/** How many streams an account allows and has open, every device counted.
 *  Mirrors `connections::Slots`. */
export interface SlotsDto {
  max: number;
  active: number;
}

/** What Settings → Account shows about the account's connections. Mirrors
 *  `ConnectionsDto`. */
export interface ConnectionsDto {
  /** Allowed: counted by the provider, or as set in "Simultaneous streams". */
  max: number | null;
  /** Open on every device, when the provider counts them; `null` when
   *  nobody can. */
  active: number | null;
  /** Streams this app holds. */
  ours: number;
  /** Whether the provider (or tuner) counted, as opposed to the setting. */
  counted: boolean;
  /** Whether "Simultaneous streams" applies: the account cannot be asked. */
  declared: boolean;
  /** Unix seconds, when the provider says. */
  expires: number | null;
}

/** One jump forward to live, automatic or asked for. */
export interface LiveJumpDto {
  /** How far forward it went, in seconds. */
  seconds: number;
  /** Counts up across the session: a jump is news when this changes, so two
   *  jumps of the same size are still two. */
  seq: number;
}

/** One audio or subtitle track of the stream that is open. Mirrors
 *  `TrackDto`. Naming it for a person is `logic/tracks.ts`. */
export interface TrackDto {
  /** mpv's id, numbered per kind: audio 1 and subtitle 1 are different
   *  tracks. */
  id: number;
  title: string | null;
  /** As the stream declares it: usually ISO 639-2 (`eng`, `fre`). */
  lang: string | null;
  /** mpv's codec name: `ac3`, `aac`, `dvb_subtitle`. */
  codec: string | null;
  /** Audio only; 6 is 5.1. */
  channels: number | null;
  default: boolean;
  forced: boolean;
  /** Subtitles for the deaf and hard of hearing. */
  hearingImpaired: boolean;
  /** Audio description. */
  visualImpaired: boolean;
  selected: boolean;
}

/** How the picture is fitted to its slot. Mirrors `AspectMode`. */
export type AspectMode = "auto" | "16:9" | "4:3" | "2.35:1" | "fill" | "stretch";

/** mpv's `deinterlace`. Mirrors `DeinterlaceMode`. */
export type DeinterlaceMode = "auto" | "on" | "off";

/** What the picture's menu offers, pushed as `EVENTS.playerOptions` when any
 *  of it changes. Mirrors `PlayerOptionsDto`. */
export interface PlayerOptionsDto {
  audio: TrackDto[];
  subtitles: TrackDto[];
  aspect: AspectMode;
  deinterlace: DeinterlaceMode;
}

/** Mirrors `EndingKind` in `src-tauri/src/ipc/ending.rs`. */
export type EndingKind =
  | "dropped"
  | "refused"
  | "notVideo"
  | "unavailable"
  | "serverError"
  | "network"
  | "noVideo"
  | "other"
  | "inUse";

export interface StreamEnding {
  kind: EndingKind;
  /** For "inUse": the account's connections as the provider counted them. */
  slots: SlotsDto | null;
  /** The HTTP status the server answered with, when it said one. */
  status: number | null;
  /** The server said in words that it was a connection limit. */
  limit: boolean;
}

/** Normalised window coordinates. Video is not a DOM element; the frontend
 *  tells Rust where to draw it. */
export interface VideoRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** How subtitles are drawn. Modelled on Vimeo's controls, not mpv's: people
 *  think in "box behind the text" and "outline". */
export type SubtitleBackdrop =
  | "background"
  | "window"
  | "outline"
  | "shadow"
  | "none";

export interface SubtitleStyleDto {
  font: string;
  /** Points at 720p; mpv scales to the window. */
  size: number;
  /** `#rrggbb`. */
  color: string;
  backdrop: SubtitleBackdrop;
  /** 0–1, defaults to 0.25. */
  backdropOpacity: number;
  outlineSize: number;
  shadowOffset: number;
}

export interface SettingsDto {
  /** Rewind buffer in megabytes (10^6 bytes): mpv's back-buffer budget as
   *  is. How long it lasts depends on the channel's bitrate. */
  rewindMb: number;
  hardwareDecode: boolean;
  /** Tone-map HDR to SDR. Off leaves HDR washed out on an SDR panel. */
  toneMapHdr: boolean;
  guideHours: number;
  syncIntervalHours: number;
  /** BCP-47 tag, or `"auto"` to follow the system. Only English ships today;
   *  the field exists so adding a locale is dropping in a catalogue rather
   *  than retrofitting every string. */
  language: string;
  subtitle: SubtitleStyleDto;
  /** Look for a new release a minute after startup and daily after that.
   *  On by default; off means only "Check for updates" asks. */
  checkForUpdates: boolean;
  /** Answer the desktop's media keys and controls (MPRIS on Linux, the
   *  SMTC on Windows). Off by default: the desktop gives those keys to one
   *  player at a time, and a music player may want to keep them. */
  mediaControls: boolean;
  /** How times of day are written: `"auto"` follows the system's locale,
   *  `"12h"` is "8:30 pm", `"24h"` is "20:30". Any other value reads as
   *  `"auto"`. */
  timeFormat: "auto" | "12h" | "24h";
  /** What a live channel does about the connect burst that starts it in the
   *  past. Mirrors `LiveStart`; "catchUp" by default. */
  liveStart: LiveStart;
  /** Streams the account allows at once, for an account that cannot say
   *  (a playlist, a Tvheadend server). Reconnecting by itself after a
   *  network failure is only safe with one to spare. Default 1. */
  simultaneousStreams: number;
}

/** Mirrors `LiveStart`: play from where the stream starts and stay there;
 *  play at once and jump to live after the connect burst; or hold the first
 *  frame and start at live. */
export type LiveStart = "stay" | "catchUp" | "wait";

/** One channel's watch history, as a backup carries it. Mirrors
 *  `ActivityRowDto`. */
export interface ActivityRowDto {
  streamId: number;
  /** The channel's name when this catalogue has it, for whoever reads the
   *  file; never used to match. */
  name: string | null;
  watchSeconds: number;
  lastWatched: number | null;
  openCount: number;
}

/** A backup opened for import. Mirrors `BackupFileDto`: the account stays in
 *  the backend, and only what it is comes across. */
export interface BackupFileDto {
  name: string;
  /** The file, without its account. Unchecked: see logic/backup.ts. */
  document: unknown;
  account: BackupAccountDto | null;
  /** Why the file's account cannot be imported, when it has one this build
   *  cannot use. */
  accountError: string | null;
}

export interface BackupAccountDto {
  kind: "xtream" | "m3u" | "hdhomerun" | "tvheadend";
  /** Where and who as, credentials redacted. */
  description: string;
  hasSecret: boolean;
}

/** What the desktop's media controls show. Mirrors `MediaSessionDto`. */
export interface MediaSessionDto {
  /** The channel. */
  title: string;
  /** What is on it, when the guide knows. */
  subtitle: string | null;
  /** The channel's logo. */
  artUrl: string | null;
  playback: MediaPlayback;
}

export type MediaPlayback = "playing" | "paused" | "stopped";

/** A media key, pushed as `EVENTS.mediaKey`. Mirrors `MediaKey`. */
export type MediaKey = "play" | "pause" | "toggle" | "next" | "previous" | "stop";

/** An item chosen in the macOS app menu that the interface carries out,
 *  pushed as `EVENTS.menu`. Mirrors `MenuCommand`. Quit is not one: Rust
 *  closes the window itself, the same close every other way out takes. */
export type MenuCommand = "settings" | "about";

/** Where a programme found by search sits relative to now, which decides what
 *  choosing it does. `catchup` is finished but still in the channel's
 *  archive; finished programmes nothing can play are never search results. */
export type AiringDto = "live" | "upcoming" | "catchup";

/** One airing of a programme, with the channel that carries it. A live hit's
 *  channel carries the programme as its `nowPlaying`. */
export interface ProgrammeHitDto {
  channel: ChannelDto;
  programme: ProgrammeDto;
  airing: AiringDto;
}

/** One channel's row in the guide, programmes included, so the grid needs no
 *  second round trip per row. */
export interface GuideRowDto {
  streamId: number;
  name: string;
  logoUrl: string | null;
  hasArchive: boolean;
  archiveDays: number | null;
  programmes: ProgrammeDto[];
}

/** A channel the viewer keeps coming back to. Earned from what is actually
 *  watched, not chosen — there is no "add to favourites" anywhere. Ordering is
 *  weighted by recency, so a channel binged months ago does not outrank one
 *  watched all week. */
export interface FavouriteDto {
  channel: ChannelDto;
  /** Total seconds watched, all time. */
  watchSeconds: number;
  /** Unix seconds. */
  lastWatched: number | null;
  openCount: number;
}

export const EVENTS = {
  syncStatus: "zapper://sync-status",
  playerState: "zapper://player-state",
  playerOptions: "zapper://player-options",
  update: "zapper://update",
  mediaKey: "zapper://media-key",
  menu: "zapper://menu",
  channelQuality: "zapper://channel-quality",
} as const;

/** A release the update dialog can show. Mirrors `UpdateReleaseDto`. */
export interface UpdateReleaseDto {
  /** `0.3.0`, `0.4.0-rc.1`. */
  version: string;
  /** The release's title, `zapper 0.3.0`. */
  name: string;
  /** Markdown from GitHub, shown as plain text: no renderer is worth
   *  shipping for a paragraph of release notes. */
  notes: string;
  /** The release page. */
  url: string;
  prerelease: boolean;
  publishedAt: string | null;
  /** This install's package. Null for a build from source, or a release
   *  without one: then only the page is offered. */
  asset: { name: string; size: number } | null;
}

/** Where the update stands, pushed as `EVENTS.update` on every change.
 *
 *  `notConfigured` is a build whose releases repository is still the
 *  placeholder: it never asks GitHub anything. A failed *check* is not a
 *  state — it leaves the previous one and is told to whoever asked. */
export type UpdateStateDto =
  | { state: "notConfigured" }
  | { state: "idle" }
  | { state: "checking" }
  | { state: "upToDate" }
  | { state: "available"; release: UpdateReleaseDto }
  | { state: "downloading"; release: UpdateReleaseDto; received: number; total: number }
  | { state: "ready"; release: UpdateReleaseDto }
  | { state: "installing"; release: UpdateReleaseDto }
  /** Waiting for the recording service to finish before installing
   *  (Windows); `until` is when it expects to, Unix seconds. */
  | { state: "waitingForRecording"; release: UpdateReleaseDto; until: number | null }
  | { state: "installed"; release: UpdateReleaseDto }
  | { state: "failed"; message: string; release: UpdateReleaseDto | null };

/** How this copy was installed. `none` is a build from source. */
export type InstallKind = "deb" | "rpm" | "arch" | "nsis" | "msi" | "dmg" | "none";

export interface UpdateStatusDto {
  /** This build's version. */
  current: string;
  kind: InstallKind;
  /** Unix seconds of the last check that got an answer, this run. */
  lastChecked: number | null;
  state: UpdateStateDto;
}

/** What Install did. `manual`: zapper could not install it itself — the
 *  command to run, why, and the package manager's output when it ran.
 *  `recording`: nothing yet, the recording service is recording and the
 *  installer would stop it; ask, then install again with `now` or
 *  `whenIdle`. `notNow`: that wait was cancelled. */
export type InstallOutcomeDto =
  | { outcome: "installed" }
  | { outcome: "quitting" }
  | { outcome: "manual"; command: string; reason: string; output: string | null }
  | { outcome: "recording"; until: number | null; title: string | null }
  | { outcome: "notNow" };

/** What Install may do about a recording in progress. */
export type InstallWhen = "ask" | "now" | "whenIdle";

/** A scrollback preview: what the picture looked like at a point in the
 *  buffer. `behindLive` is the moment it was actually taken, which is not
 *  quite the moment that was asked for — the frames are sampled every few
 *  seconds, and the interface says which one it is showing. */
export interface ThumbnailDto {
  dataUrl: string;
  behindLive: number;
}

/** One reading for the diagnostics panel — "stats for nerds".
 *
 *  Polled rather than pushed: it is far more than the state feed needs and
 *  nothing should pay for it while the panel is closed.
 *
 *  Carries no stream URL and no credentials. The panel exists to be
 *  screenshotted into a bug report, and an Xtream URL has the account's
 *  username and password in its query string.
 *
 *  Everything is nullable because mpv drops most of it between files and
 *  half of it never exists for a raw MPEG-TS multiplex. Null means "this
 *  stream does not say", drawn as a dash. */
export interface StatsDto {
  container: string | null;
  videoCodec: string | null;
  audioCodec: string | null;
  hwdec: string | null;
  pixelFormat: string | null;
  primaries: string | null;
  gamma: string | null;
  videoBitrate: number | null;
  audioBitrate: number | null;
  measuredFps: number | null;
  containerFps: number | null;
  audioSamplerate: number | null;
  audioChannels: number | null;
  cacheDuration: number | null;
  cacheSpeed: number | null;
  demuxerBytes: number | null;
  buffering: boolean | null;
  cacheBufferingPercent: number | null;
  droppedFrames: number | null;
  decoderDroppedFrames: number | null;
  avsync: number | null;
  behindLive: number;
  bufferedBack: number;
  /** The last minute of health, oldest first. Collected by the backend on
   *  the player's own heartbeat rather than by whoever is asking, so the
   *  graphs are already drawn the moment the panel opens. */
  history: StatsSampleDto[];
  /** Times playback stopped to wait for data since the stream started. A
   *  stall drops no frames, so the dropped-frame counters never see it. */
  stalls: number;
}

/** One point on a diagnostics graph. Mirrors `ipc::health::SampleDto`. */
export interface StatsSampleDto {
  /** Seconds of demuxed data ahead of the playhead. */
  buffer: number;
  /** Bytes per second arriving. */
  speed: number;
  /** Frames dropped since the previous sample — a difference, not a running
   *  total, because a total only ever climbs and says nothing about when. */
  dropped: number;
  /** Whether playback was stopped waiting for data at this instant. */
  starved: boolean;
}

/** The account, as much of it as this window may see.
 *
 *  No password. It is write-only: it can be set and replaced, but it is
 *  never sent back, so it cannot leak through a devtools inspector, a
 *  screenshot of one, or a serialised state dump. */
export interface AccountDto {
  host: string;
  username: string;
  /** Whether one is on file — enough to draw the field as "set". */
  hasPassword: boolean;
  /** "zapper" when an account is on file, "none" when there is not one
   *  yet. */
  source: string;
  /** Where it is kept, so "where is this?" is answerable on screen. */
  path: string;
  /** Which set of fields is in use. Every set is reported, so switching
   *  kinds in the pane shows the others filled in as they were left. */
  kind: "xtream" | "m3u" | "hdhomerun" | "tvheadend";
  /** The playlist location, with any credentials in it redacted. */
  playlist: string;
  /** True when `playlist` had credentials taken out: it is shown as set and
   *  kept unless a new one is typed, like the password. */
  playlistRedacted: boolean;
  /** XMLTV locations, redacted the same way. */
  epgUrls: string[];
  epgRedacted: boolean;
  /** Empty for the default. */
  userAgent: string;
  /** The HDHomeRun's address, as typed. */
  hdhomerunDevice: string;
  /** XMLTV guides used instead of SiliconDust's; empty for SiliconDust's.
   *  Redacted like the playlist's guides. */
  hdhomerunEpgUrls: string[];
  hdhomerunEpgRedacted: boolean;
  /** The Tvheadend server and login. The password is write-only, like the
   *  Xtream one: only whether one is on file. */
  tvheadendUrl: string;
  tvheadendUsername: string;
  tvheadendHasPassword: boolean;
}

/** An account as typed and not saved, for the connection test. Each kind
 *  carries what its save command takes, with the same rules: an empty
 *  password and a `null` list keep what is on file. */
export type AccountCandidateDto =
  | { kind: "xtream"; host: string; username: string; password: string }
  | { kind: "m3u"; playlist: string | null; epgUrls: string[] | null; userAgent: string }
  | { kind: "hdhomerun"; device: string; epgUrls: string[] | null }
  | { kind: "tvheadend"; url: string; username: string; password: string };

// ------------------------------------------------------------- recording
// Mirror of src-tauri/src/recording/api.rs and recording/service.rs: what the
// recording service (zapper-server) answers, passed through the app's Rust
// side unchanged.

/** `finishing`: no longer capturing (stopped, or its window over) and being
 *  turned into its files, which for a long recording takes a while. */
export type RecordingState = "scheduled" | "recording" | "finishing" | "done" | "failed";

export interface PartDto {
  /** Absolute path: a `.ts` while capturing, the `.mkv` once finished. */
  path: string;
  /** The feed it came from: the main channel or the backup. */
  streamId: number;
  bytes: number;
  started: number;
  ended: number | null;
}

export interface RecordingDto {
  id: number;
  streamId: number;
  backupStreamId: number | null;
  channelName: string;
  title: string;
  episodeTitle?: string;
  season?: number;
  episode?: number;
  /** The programme, Unix seconds. */
  start: number;
  stop: number;
  /** Seconds recorded before the start and after the end. */
  padBefore: number;
  padAfter: number;
  state: RecordingState;
  parts: PartDto[];
  notes: string[];
  error?: string;
  folder: string;
  /** The backup's channel name, when the catalogue has it. */
  backupName?: string;
  /** While recording: why it is not capturing right now. */
  waiting?: string;
  /** What it would lose to recordings on other channels when the account
   *  has too few connections for them all; absent when nothing. */
  conflicts?: ConflictDto[];
}

/** Why a channel is offered as a recording's backup: it carries the same
 *  guide channel, or its guide has the same programme at the same time. */
export type BackupReason = "guide" | "programme";

/** A channel a recording could fall back to (src-tauri/src/recording/backup.rs). */
export interface BackupDto {
  streamId: number;
  name: string;
  num?: number;
  numLabel?: string;
  reason: BackupReason;
  /** The last measured picture height, when it has been played. */
  height?: number;
  /** The last time it was opened, it failed to load. */
  failing?: boolean;
}

/** A recording's backup now, and the candidates, best first. */
export interface BackupsDto {
  current?: number;
  candidates: BackupDto[];
}

/** What a recording loses to another one on another channel. */
export type ConflictKind = "cutEnd" | "lateStart" | "lostStart" | "missed";

export interface ConflictDto {
  /** The recording that loses, and the one it loses to. */
  title: string;
  with: string;
  kind: ConflictKind;
  /** How much it loses; 0 for `missed`. */
  secs: number;
  /** How many streams the account allows. */
  streams: number;
  /** In a preview: the loss is another recording's, not the one asked
   *  about. */
  other?: boolean;
}

export interface ScheduleRequest {
  streamId: number;
  /** Omitted: the service picks another channel with the same guide channel. */
  backupStreamId?: number | null;
  channelName?: string | null;
  title: string;
  episodeTitle?: string | null;
  season?: number | null;
  episode?: number | null;
  start: number;
  stop: number;
  padBefore?: number | null;
  padAfter?: number | null;
}

/** How recordings are named and grouped in folders (src/logic/naming.ts). */
export type Naming = "mediaServer" | "byChannel" | "byDate" | "flat" | "custom";

/** What other programs see of a recording while it records
 *  (src-tauri/src/recording/api.rs). */
export type WhileRecording = "hidden" | "visible" | "strm";

export interface RecordingSettingsDto {
  outputDir: string;
  naming: Naming;
  /** The custom pattern, kept while a preset is chosen. Empty, or one the
   *  service accepts. */
  namingPattern: string;
  /** Seconds. */
  padBefore: number;
  padAfter: number;
  clockTrack: boolean;
  /** What other programs (a media server) see of a recording while it
   *  records: nothing, the growing file, or a `.strm` for Jellyfin. */
  whileRecording?: WhileRecording;
  /** `whileRecording` is "visible": for clients that know only the on/off
   *  setting it was. */
  showWhileRecording?: boolean;
  /** The host `.strm` files point at; empty for the service's choice. */
  strmHost?: string;
  /** Where `.strm` files point now (`https://192.168.1.20:8796`). Read
   *  only. */
  strmBase?: string;
  /** The folder used when none is chosen; an empty `outputDir` sent back
   *  means this one. */
  defaultOutputDir: string;
}

export interface ActiveRecordingDto {
  id: number;
  title: string;
  streamId: number;
  bytes: number;
  since: number;
  waiting?: string;
  /** When it stops, padding included (Unix seconds). */
  until: number;
  /** No longer capturing: being finished into its files. */
  finishing?: boolean;
}

export interface RecordingStatusDto {
  version: string;
  apiVersion: number;
  active: ActiveRecordingDto[];
  outputDir: string;
  diskFree: number | null;
  diskTotal: number | null;
  now: number;
  /** Nothing recording, nothing starting within five minutes. */
  idle: boolean;
  /** When it next will be idle, if it is not now. */
  idleAt?: number;
  /** The next padded start of a recording not yet begun. */
  nextStart?: number;
}

export interface ServiceStatusDto {
  supported: boolean;
  unsupportedReason?: string;
  /** What Install sets up, in words: "a systemd user service". */
  kind?: string;
  installed: boolean;
  /** systemd's word (active, inactive, failed…), or Task Scheduler's in the
   *  same spirit (running, stopped, disabled). */
  state?: string;
  /** Keeps running after logging out; on Windows, installed as the Windows
   *  service that starts at boot. */
  linger: boolean;
  /** Whether keeping it running after logging out is offered here (not on
   *  macOS, where the LaunchAgent runs while you are logged in). */
  canLinger: boolean;
  /** Keeping it running is the Windows service that starts at boot, as an
   *  account of its own, installed with an administrator's permission. */
  bootService: boolean;
  binary?: string;
  /** Set when the service answered on its API. */
  version?: string;
  address: string;
  versionMismatch: boolean;
}

/** Network access (Settings › Recording), as it is changed. */
export interface NetworkSettingsDto {
  /** Listen on every interface, not only 127.0.0.1. Needs a password set. */
  enabled: boolean;
  /** 1024–65535, for the app and the network alike. */
  port: number;
  /** Other devices get HTTPS; off is plain HTTP on the network. */
  https: boolean;
}

/** A device signed in with the password. */
export interface DeviceDto {
  id: string;
  name: string;
  userAgent: string;
  /** Unix seconds. */
  created: number;
  lastUsed?: number;
  /** Where it was last seen from. */
  address?: string;
}

export interface NetworkDto extends NetworkSettingsDto {
  /** The password itself never leaves the service. */
  passwordSet: boolean;
  /** The HTTPS certificate's SHA-256, `AB:CD:…`, once one was made. */
  fingerprint?: string;
  /** `https://192.168.1.20:8796`, one per network address of the computer. */
  addresses: string[];
  /** Why network access is not on as set. */
  error?: string;
  /** Most recently used first. */
  devices: DeviceDto[];
}

// ------------------------------------------------------------------ movies
// Mirrors the movie DTOs at the end of `src-tauri/src/ipc/types.rs`.

export interface VodCategoryDto {
  id: string;
  name: string;
  movieCount: number;
}

export interface MovieDto {
  movieId: number;
  name: string;
  cleanName: string;
  year: number | null;
  /** Out of ten. */
  rating: number | null;
  posterUrl: string | null;
  /** Unix seconds. */
  added: number | null;
  durationSecs: number | null;
  categoryId: string | null;
  favourite: boolean;
  watched: boolean;
}

export type MovieSort = "added" | "az" | "year" | "rating";
export type MovieListKind = "favourites" | "recent" | "watched" | "category";

export interface MovieQueryDto {
  list: MovieListKind;
  /** For `list: "category"`. */
  categoryId: string | null;
  sort: MovieSort;
  yearFrom: number | null;
  yearTo: number | null;
}

export interface MoviePageDto {
  /** In the whole list, not just this page. */
  total: number;
  items: MovieDto[];
}

export interface MovieDetailsDto {
  movie: MovieDto;
  plot: string | null;
  genres: string[];
  director: string | null;
  cast: string[];
  releaseDate: string | null;
  backdropUrls: string[];
  tmdbId: number | null;
  audioLanguages: string[];
  subtitleLanguages: string[];
  /** Unix seconds; `null` when there are no details to show. */
  fetchedAt: number | null;
}

export interface MoviesSummaryDto {
  /** The tab is shown only when the account has movies. */
  available: boolean;
  note: string | null;
  total: number;
  favourites: number;
  recent: number;
  watched: number;
}

export interface MarksDto {
  id: number;
  favourite: boolean;
  watched: boolean;
  position: number | null;
  duration: number | null;
}

export type OnDemandKind = "movie";

export interface OnDemandDto {
  kind: OnDemandKind;
  id: number;
  /** Seconds from the start. */
  position: number;
  duration: number | null;
  /** How far the data has been read ahead, seconds from the start. */
  bufferedTo: number | null;
}
