// Thin typed wrapper over the Tauri command boundary.
//
// Every call can fail — the backend returns Result<_, String> — so callers get
// a rejected promise with a plain message, never a panic.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  EVENTS,
  type AccountCandidateDto,
  type AccountDto,
  type ActivityRowDto,
  type BackupFileDto,
  type CategoryDto,
  type ChannelDto,
  type ChannelQualityDto,
  type PlayerStateDto,
  type PlayerOptionsDto,
  type AspectMode,
  type ConnectionsDto,
  type MarksDto,
  type MovieDetailsDto,
  type MovieDto,
  type MoviePageDto,
  type MovieQueryDto,
  type MoviesSummaryDto,
  type VodCategoryDto,
  type DeinterlaceMode,
  type ProgrammeDto,
  type ProgrammeHitDto,
  type SettingsDto,
  type SubtitleStyleDto,
  type FavouriteDto,
  type GuideRowDto,
  type SyncStatusDto,
  type StatsDto,
  type ThumbnailDto,
  type InstallOutcomeDto,
  type InstallWhen,
  type MediaKey,
  type MenuCommand,
  type MediaSessionDto,
  type UpdateStatusDto,
  type VideoRect,
  type NetworkDto,
  type NetworkSettingsDto,
  type BackupsDto,
  type RecordingDto,
  type ConflictDto,
  type RecordingSettingsDto,
  type RecordingStatusDto,
  type ScheduleRequest,
  type ServiceStatusDto,
} from "./types";

export const api = {
  categories: () => invoke<CategoryDto[]>("categories"),

  channels: (categoryId: string | null, offset: number, limit: number) =>
    invoke<ChannelDto[]>("channels", { categoryId, offset, limit }),

  channelsByIds: (ids: number[]) => invoke<ChannelDto[]>("channels_by_ids", { ids }),

  /** Every channel number as the list writes it ("12", "5.1") and the channel
   *  it tunes, in list order; see logic/zap.ts. */
  channelNumbers: () => invoke<[string, number][]>("channel_numbers"),

  search: (query: string, limit: number) =>
    invoke<ChannelDto[]>("search", { query, limit }),

  /** Airings whose title matches: on now, then upcoming, then catch-up. */
  searchProgrammes: (query: string, offset: number, limit: number) =>
    invoke<ProgrammeHitDto[]>("search_programmes", { query, offset, limit }),

  /** Where a channel sits in a category's listing; `null` when it is not in
   *  it. `categoryId` null is every channel. */
  channelIndex: (categoryId: string | null, streamId: number) =>
    invoke<number | null>("channel_index", { categoryId, streamId }),

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
  /** Plays a movie, from `start` seconds or, with `null`, from the start. */
  playMovie: (movieId: number, start: number | null) =>
    invoke<void>("play_movie", { movieId, start }),
  /** To an absolute position in a movie, in seconds. */
  seekTo: (seconds: number) => invoke<void>("seek_to", { seconds }),
  setVolume: (volume: number) => invoke<void>("set_volume", { volume }),
  setMuted: (muted: boolean) => invoke<void>("set_muted", { muted }),
  setCompressor: (enabled: boolean) =>
    invoke<void>("set_compressor", { enabled }),
  setVideoRect: (rect: VideoRect) => invoke<void>("set_video_rect", { rect }),
  playerState: () => invoke<PlayerStateDto>("player_state"),
  playerStats: () => invoke<StatsDto>("player_stats"),
  playerOptions: () => invoke<PlayerOptionsDto>("player_options"),
  setAudioTrack: (id: number) => invoke<PlayerOptionsDto>("set_audio_track", { id }),
  /** `null` turns subtitles off. */
  setSubtitleTrack: (id: number | null) =>
    invoke<PlayerOptionsDto>("set_subtitle_track", { id }),
  setAspect: (aspect: AspectMode) => invoke<PlayerOptionsDto>("set_aspect", { aspect }),
  setDeinterlace: (mode: DeinterlaceMode) =>
    invoke<PlayerOptionsDto>("set_deinterlace", { mode }),
  thumbnailAt: (behindLive: number) =>
    invoke<ThumbnailDto | null>("thumbnail_at", { behindLive }),

  account: () => invoke<AccountDto>("account"),
  setAccount: (host: string, username: string, password: string) =>
    invoke<void>("set_account", { host, username, password }),
  /** `null` keeps what is on file — the redacted playlist or guides. */
  setM3uAccount: (playlist: string | null, epgUrls: string[] | null, userAgent: string) =>
    invoke<void>("set_m3u_account", { playlist, epgUrls, userAgent }),
  /** `null` keeps the guide list on file, which was shown redacted. */
  setHdhomerunAccount: (device: string, epgUrls: string[] | null) =>
    invoke<void>("set_hdhomerun_account", { device, epgUrls }),
  /** An empty password keeps the one on file; an empty username is a server
   *  that needs no login. */
  setTvheadendAccount: (url: string, username: string, password: string) =>
    invoke<void>("set_tvheadend_account", { url, username, password }),
  testAccount: () => invoke<string>("test_account"),
  /** Tests an account as typed, without saving it or touching the one in
   *  use. */
  testAccountCandidate: (candidate: AccountCandidateDto) =>
    invoke<string>("test_account_candidate", { candidate }),

  settings: () => invoke<SettingsDto>("settings"),
  setSettings: (settings: SettingsDto) => invoke<void>("set_settings", { settings }),
  setSubtitleStyle: (style: SubtitleStyleDto) =>
    invoke<void>("set_subtitle_style", { style }),
  applyPlaybackSettings: (settings: SettingsDto) =>
    invoke<void>("apply_playback_settings", { settings }),
  /** Stops the reconnect the backend is making by itself after a network
   *  failure; mid-attempt, the attempt too. */
  cancelReconnect: () => invoke<void>("cancel_reconnect"),
  /** The account's connections: allowed, open, when it expires. */
  connectionStatus: () => invoke<ConnectionsDto>("connection_status"),
  /** What the desktop's media controls show; `null` with nothing on. */
  setMediaSession: (session: MediaSessionDto | null) =>
    invoke<void>("set_media_session", { session }),
  guideRows: (streamIds: number[], from: number, to: number) =>
    invoke<GuideRowDto[]>("guide_rows", { streamIds, from, to }),

  favourites: (limit: number) => invoke<FavouriteDto[]>("favourites", { limit }),
  recentlyWatched: (limit: number) => invoke<FavouriteDto[]>("recently_watched", { limit }),
  forgetActivity: (streamId: number) =>
    invoke<void>("forget_activity", { streamId }),

  moviesSummary: () => invoke<MoviesSummaryDto>("movies_summary"),
  movieCategories: () => invoke<VodCategoryDto[]>("movie_categories"),
  movies: (query: MovieQueryDto, offset: number, limit: number) =>
    invoke<MoviePageDto>("movies", { query, offset, limit }),
  searchMovies: (query: string, limit: number) =>
    invoke<MovieDto[]>("search_movies", { query, limit }),
  /** Fetched from the provider the first time and kept for a week. */
  movieDetails: (movieId: number) => invoke<MovieDetailsDto>("movie_details", { movieId }),
  similarMovies: (movieId: number, limit: number) =>
    invoke<MovieDto[]>("similar_movies", { movieId, limit }),
  /** An absent mark is left as it is. */
  setMovieMarks: (movieId: number, marks: { favourite?: boolean; watched?: boolean }) =>
    invoke<MarksDto>("set_movie_marks", {
      movieId,
      favourite: marks.favourite ?? null,
      watched: marks.watched ?? null,
    }),

  // Backups (Settings › Data). The backend opens the file dialogs itself, so
  // no path crosses this boundary, and it adds and removes the account, so
  // no password does either.
  activityRows: () => invoke<ActivityRowDto[]>("activity_rows"),
  importActivity: (rows: ActivityRowDto[]) => invoke<number>("import_activity", { rows }),
  /** Asks where to save; the file's name, or `null` if cancelled. */
  exportBackup: (document: unknown, includeAccount: boolean, fileName: string) =>
    invoke<string | null>("export_backup", { document, includeAccount, fileName }),
  /** Asks for a file and reads it; `null` if cancelled. */
  openBackup: () => invoke<BackupFileDto | null>("open_backup"),
  /** Writes the account from the backup opened last. */
  importBackupAccount: () => invoke<void>("import_backup_account"),

  // The updater. None of these takes a URL or a path: what to fetch, install
  // and open is decided by the backend from its own state.
  updateStatus: () => invoke<UpdateStatusDto>("update_status"),
  updateCheck: () => invoke<UpdateStatusDto>("update_check"),
  updateDownload: () => invoke<UpdateStatusDto>("update_download"),
  updateCancel: () => invoke<UpdateStatusDto>("update_cancel"),
  updateInstall: (when: InstallWhen = "ask") =>
    invoke<InstallOutcomeDto>("update_install", { when }),
  updateRestart: () => invoke<void>("update_restart"),
  updateOpenPage: () => invoke<void>("update_open_page"),

  // The recording service, asked through the Rust side (which holds its
  // token). Each rejects with the service's words, or "The recording service
  // is not running." when nothing answers.
  recordings: () => invoke<RecordingDto[]>("recordings"),
  recordProgramme: (request: ScheduleRequest) =>
    invoke<RecordingDto>("record_programme", { request }),
  previewRecording: (request: ScheduleRequest) =>
    invoke<ConflictDto[]>("preview_recording", { request }),
  cancelRecording: (id: number) => invoke<void>("cancel_recording", { id }),
  recordingBackups: (id: number) => invoke<BackupsDto>("recording_backups", { id }),
  setRecordingBackup: (id: number, streamId: number | null) =>
    invoke<RecordingDto>("set_recording_backup", { id, streamId }),
  deleteRecording: (id: number, files: boolean) =>
    invoke<void>("delete_recording", { id, files }),
  recordingStatus: () => invoke<RecordingStatusDto>("recording_status"),
  recordingSettings: () => invoke<RecordingSettingsDto>("recording_settings"),
  setRecordingSettings: (settings: RecordingSettingsDto) =>
    invoke<RecordingSettingsDto>("set_recording_settings", { settings }),
  recordingNetwork: () => invoke<NetworkDto>("recording_network"),
  setRecordingNetwork: (settings: NetworkSettingsDto) =>
    invoke<NetworkDto>("set_recording_network", { settings }),
  setRecordingPassword: (password: string) =>
    invoke<NetworkDto>("set_recording_password", { password }),
  revokeRecordingDevice: (id: string) => invoke<NetworkDto>("revoke_recording_device", { id }),
  recordingService: () => invoke<ServiceStatusDto>("recording_service"),
  installRecordingService: (linger: boolean) =>
    invoke<ServiceStatusDto>("install_recording_service", { linger }),
  removeRecordingService: () => invoke<ServiceStatusDto>("remove_recording_service"),
  setRecordingLinger: (on: boolean) => invoke<ServiceStatusDto>("set_recording_linger", { on }),
  openRecordingFolder: (id: number) => invoke<void>("open_recording_folder", { id }),
};

export const onSyncStatus = (f: (s: SyncStatusDto) => void): Promise<UnlistenFn> =>
  listen<SyncStatusDto>(EVENTS.syncStatus, (e) => f(e.payload));

export const onPlayerState = (f: (s: PlayerStateDto) => void): Promise<UnlistenFn> =>
  listen<PlayerStateDto>(EVENTS.playerState, (e) => f(e.payload));

export const onPlayerOptions = (f: (s: PlayerOptionsDto) => void): Promise<UnlistenFn> =>
  listen<PlayerOptionsDto>(EVENTS.playerOptions, (e) => f(e.payload));

export const onUpdate = (f: (s: UpdateStatusDto) => void): Promise<UnlistenFn> =>
  listen<UpdateStatusDto>(EVENTS.update, (e) => f(e.payload));

export const onMediaKey = (f: (k: MediaKey) => void): Promise<UnlistenFn> =>
  listen<MediaKey>(EVENTS.mediaKey, (e) => f(e.payload));

export const onMenu = (f: (c: MenuCommand) => void): Promise<UnlistenFn> =>
  listen<MenuCommand>(EVENTS.menu, (e) => f(e.payload));

export const onChannelQuality = (f: (q: ChannelQualityDto) => void): Promise<UnlistenFn> =>
  listen<ChannelQualityDto>(EVENTS.channelQuality, (e) => f(e.payload));
