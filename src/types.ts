// Mirror of src-tauri/src/ipc/types.rs. Change both together or not at all.
// Rust serialises camelCase, so these names match the wire exactly.

export interface CategoryDto {
  id: string;
  name: string;
  /** Excludes separators and soft-deleted rows. */
  channelCount: number;
}

export interface QualityDto {
  codec: string | null;
  width: number | null;
  height: number | null;
  fps: number | null;
  isHdr: boolean;
  okCount: number;
  failCount: number;
}

export interface ProgrammeDto {
  /** Unix seconds. */
  start: number;
  stop: number;
  title: string;
  description: string | null;
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
  /** The last time playback skipped ahead to live by itself on this stream —
   *  after a tune-in's connect burst, or after stalls — or null. Cleared by
   *  the next open. */
  liveJump: LiveJumpDto | null;
}

/** One automatic catch-up to live. */
export interface LiveJumpDto {
  /** How far forward it went, in seconds. */
  seconds: number;
  /** Counts up across the session: a jump is news when this changes, so two
   *  jumps of the same size are still two. */
  seq: number;
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
  update: "zapper://update",
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
  | { state: "installed"; release: UpdateReleaseDto }
  | { state: "failed"; message: string; release: UpdateReleaseDto | null };

/** How this copy was installed. `none` is a build from source. */
export type InstallKind = "deb" | "rpm" | "nsis" | "msi" | "dmg" | "none";

export interface UpdateStatusDto {
  /** This build's version. */
  current: string;
  kind: InstallKind;
  /** Unix seconds of the last check that got an answer, this run. */
  lastChecked: number | null;
  state: UpdateStateDto;
}

/** What Install did. `manual`: zapper could not install it itself — the
 *  command to run, why, and the package manager's output when it ran. */
export type InstallOutcomeDto =
  | { outcome: "installed" }
  | { outcome: "quitting" }
  | { outcome: "manual"; command: string; reason: string; output: string | null };

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
