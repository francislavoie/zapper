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
  volume: number;
  muted: boolean;
  compressor: boolean;
  error: string | null;
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
  /** Minutes of live rewind; converted to mpv's byte budget behind the scenes. */
  rewindMinutes: number;
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

export const EVENTS = {
  syncStatus: "zapper://sync-status",
  playerState: "zapper://player-state",
} as const;
