// Thin typed wrapper over the Tauri command boundary.
//
// Every call can fail — the backend returns Result<_, String> — so callers get
// a rejected promise with a plain message, never a panic.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  EVENTS,
  type CategoryDto,
  type ChannelDto,
  type PlayerStateDto,
  type ProgrammeDto,
  type SettingsDto,
  type SubtitleStyleDto,
  type GuideRowDto,
  type SyncStatusDto,
  type VideoRect,
} from "./types";

export const api = {
  categories: () => invoke<CategoryDto[]>("categories"),

  channels: (categoryId: string | null, offset: number, limit: number) =>
    invoke<ChannelDto[]>("channels", { categoryId, offset, limit }),

  search: (query: string, limit: number) =>
    invoke<ChannelDto[]>("search", { query, limit }),

  guide: (streamIds: number[], from: number, to: number) =>
    invoke<[number, ProgrammeDto[]][]>("guide", { streamIds, from, to }),

  syncStatus: () => invoke<SyncStatusDto>("sync_status"),
  syncNow: () => invoke<void>("sync_now"),

  play: (streamId: number) => invoke<void>("play", { streamId }),
  playCatchup: (streamId: number, start: number, durationMinutes: number) =>
    invoke<void>("play_catchup", { streamId, start, durationMinutes }),
  stop: () => invoke<void>("stop"),
  togglePause: () => invoke<void>("toggle_pause"),
  seekRelative: (seconds: number) => invoke<void>("seek_relative", { seconds }),
  seekLive: () => invoke<void>("seek_live"),
  setVolume: (volume: number) => invoke<void>("set_volume", { volume }),
  setMuted: (muted: boolean) => invoke<void>("set_muted", { muted }),
  setCompressor: (enabled: boolean) =>
    invoke<void>("set_compressor", { enabled }),
  setVideoRect: (rect: VideoRect) => invoke<void>("set_video_rect", { rect }),
  playerState: () => invoke<PlayerStateDto>("player_state"),

  settings: () => invoke<SettingsDto>("settings"),
  setSettings: (settings: SettingsDto) => invoke<void>("set_settings", { settings }),
  setSubtitleStyle: (style: SubtitleStyleDto) =>
    invoke<void>("set_subtitle_style", { style }),
  applyPlaybackSettings: (settings: SettingsDto) =>
    invoke<void>("apply_playback_settings", { settings }),
  guideRows: (streamIds: number[], from: number, to: number) =>
    invoke<GuideRowDto[]>("guide_rows", { streamIds, from, to }),
};

export const onSyncStatus = (f: (s: SyncStatusDto) => void): Promise<UnlistenFn> =>
  listen<SyncStatusDto>(EVENTS.syncStatus, (e) => f(e.payload));

export const onPlayerState = (f: (s: PlayerStateDto) => void): Promise<UnlistenFn> =>
  listen<PlayerStateDto>(EVENTS.playerState, (e) => f(e.payload));
