// Thin typed wrapper over the Tauri command boundary.
//
// Every call can fail — the backend returns Result<_, String> — so callers get
// a rejected promise with a plain message, never a panic.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  EVENTS,
  type AccountDto,
  type CategoryDto,
  type ChannelDto,
  type PlayerStateDto,
  type ProgrammeDto,
  type SettingsDto,
  type SubtitleStyleDto,
  type FavouriteDto,
  type GuideRowDto,
  type SyncStatusDto,
  type StatsDto,
  type ThumbnailDto,
  type InstallOutcomeDto,
  type UpdateStatusDto,
  type VideoRect,
} from "./types";

export const api = {
  categories: () => invoke<CategoryDto[]>("categories"),

  channels: (categoryId: string | null, offset: number, limit: number) =>
    invoke<ChannelDto[]>("channels", { categoryId, offset, limit }),

  channelsByIds: (ids: number[]) => invoke<ChannelDto[]>("channels_by_ids", { ids }),

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
  playerStats: () => invoke<StatsDto>("player_stats"),
  thumbnailAt: (behindLive: number) =>
    invoke<ThumbnailDto | null>("thumbnail_at", { behindLive }),

  account: () => invoke<AccountDto>("account"),
  setAccount: (host: string, username: string, password: string) =>
    invoke<void>("set_account", { host, username, password }),
  /** `null` keeps what is on file — the redacted playlist or guides. */
  setM3uAccount: (playlist: string | null, epgUrls: string[] | null, userAgent: string) =>
    invoke<void>("set_m3u_account", { playlist, epgUrls, userAgent }),
  testAccount: () => invoke<string>("test_account"),

  settings: () => invoke<SettingsDto>("settings"),
  setSettings: (settings: SettingsDto) => invoke<void>("set_settings", { settings }),
  setSubtitleStyle: (style: SubtitleStyleDto) =>
    invoke<void>("set_subtitle_style", { style }),
  applyPlaybackSettings: (settings: SettingsDto) =>
    invoke<void>("apply_playback_settings", { settings }),
  guideRows: (streamIds: number[], from: number, to: number) =>
    invoke<GuideRowDto[]>("guide_rows", { streamIds, from, to }),

  favourites: (limit: number) => invoke<FavouriteDto[]>("favourites", { limit }),
  forgetActivity: (streamId: number) =>
    invoke<void>("forget_activity", { streamId }),

  // The updater. None of these takes a URL or a path: what to fetch, install
  // and open is decided by the backend from its own state.
  updateStatus: () => invoke<UpdateStatusDto>("update_status"),
  updateCheck: () => invoke<UpdateStatusDto>("update_check"),
  updateDownload: () => invoke<UpdateStatusDto>("update_download"),
  updateCancel: () => invoke<UpdateStatusDto>("update_cancel"),
  updateInstall: () => invoke<InstallOutcomeDto>("update_install"),
  updateRestart: () => invoke<void>("update_restart"),
  updateOpenPage: () => invoke<void>("update_open_page"),
};

export const onSyncStatus = (f: (s: SyncStatusDto) => void): Promise<UnlistenFn> =>
  listen<SyncStatusDto>(EVENTS.syncStatus, (e) => f(e.payload));

export const onPlayerState = (f: (s: PlayerStateDto) => void): Promise<UnlistenFn> =>
  listen<PlayerStateDto>(EVENTS.playerState, (e) => f(e.payload));

export const onUpdate = (f: (s: UpdateStatusDto) => void): Promise<UnlistenFn> =>
  listen<UpdateStatusDto>(EVENTS.update, (e) => f(e.payload));
