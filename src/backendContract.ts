// What every backend offers: the calls the interface makes. The web backend
// (src/web.ts) implements them over HTTP, the mock (src/mock.ts) in memory.

import type {
  AccountCandidateDto,
  AccountDto,
  ActivityRowDto,
  BackupFileDto,
  CategoryDto,
  ChannelDto,
  PlayerStateDto,
  PlayerOptionsDto,
  AspectMode,
  ConnectionsDto,
  ContinueItemDto,
  ContinueKind,
  MarksDto,
  MovieDetailsDto,
  MovieDto,
  MoviePageDto,
  MovieQueryDto,
  MoviesSummaryDto,
  PlayingEpisodeDto,
  SeriesCategoryDto,
  SeriesDetailsDto,
  SeriesPageDto,
  SeriesQueryDto,
  ShowsSummaryDto,
  VodCategoryDto,
  DeinterlaceMode,
  ProgrammeDto,
  ProgrammeHitDto,
  SettingsDto,
  SubtitleStyleDto,
  FavouriteDto,
  GuideRowDto,
  SyncStatusDto,
  StatsDto,
  ThumbnailDto,
  TitleThumbnailDto,
  InstallOutcomeDto,
  InstallWhen,
  MediaSessionDto,
  UpdateStatusDto,
  VideoRect,
  NetworkDto,
  NetworkSettingsDto,
  BackupsDto,
  RecordingDto,
  ConflictDto,
  RecordingSettingsDto,
  RecordingStatusDto,
  ScheduleRequest,
  ServiceStatusDto,
  OnDemandKind,
  OnlineServicesDto,
  SubtitleSearchDto,
  SubtitleDownloadDto,
  KeptSubtitleDto,
} from "./types";

export interface Backend {
  categories: () => Promise<CategoryDto[]>;
  channels: (categoryId: string | null, offset: number, limit: number) => Promise<ChannelDto[]>;
  channelsByIds: (ids: number[]) => Promise<ChannelDto[]>;
  /** Other channels carrying the same programmes as this one (the same
   *  guide id), to try when it is off air: those that last played first, at
   *  most three, none that last failed. Empty when there are none. */
  alternativesFor: (streamId: number) => Promise<ChannelDto[]>;
  /** Every channel number as the list writes it ("12", "5.1") and the channel
   *  it tunes, in list order; see logic/zap.ts. */
  channelNumbers: () => Promise<[string, number][]>;
  search: (query: string, limit: number) => Promise<ChannelDto[]>;
  /** Airings whose title matches: on now, then upcoming, then catch-up. */
  searchProgrammes: (query: string, offset: number, limit: number) => Promise<ProgrammeHitDto[]>;
  /** Where a channel sits in a category's listing; `null` when it is not in
   *  it. `categoryId` null is every channel. */
  channelIndex: (categoryId: string | null, streamId: number) => Promise<number | null>;
  guide: (streamIds: number[], from: number, to: number) => Promise<[number, ProgrammeDto[]][]>;
  syncStatus: () => Promise<SyncStatusDto>;
  syncNow: () => Promise<void>;
  play: (streamId: number) => Promise<void>;
  playCatchup: (streamId: number, start: number, durationMinutes: number) => Promise<void>;
  stop: () => Promise<void>;
  togglePause: () => Promise<void>;
  /** Pauses or plays, as `paused` says: asking twice is asking once. Playing
   *  a title opened held ends its hold. */
  setPaused: (paused: boolean) => Promise<void>;
  seekRelative: (seconds: number) => Promise<void>;
  seekLive: () => Promise<void>;
  /** Plays a movie, from `start` seconds or, with `null`, from the start.
   *  `held` opens it paused there, for a question to be asked. */
  playMovie: (movieId: number, start: number | null, held?: boolean) => Promise<void>;
  /** Plays an episode, from `start` seconds or, with `null`, from the start.
   *  `held` opens it paused there, for a question to be asked. */
  playEpisode: (episodeId: number, start: number | null, held?: boolean) => Promise<void>;
  /** Up next: moves on from the episode playing, or the one that just played
   *  to its end, to the next, from its start, marking the first watched.
   *  Refused once anything else is open, after a drop, or after a stop. */
  playNextEpisode: (fromEpisodeId: number, nextEpisodeId: number) => Promise<void>;
  /** To an absolute position in a movie, in seconds. */
  seekTo: (seconds: number) => Promise<void>;
  setVolume: (volume: number) => Promise<void>;
  setMuted: (muted: boolean) => Promise<void>;
  setCompressor: (enabled: boolean) => Promise<void>;
  setVideoRect: (rect: VideoRect) => Promise<void>;
  playerState: () => Promise<PlayerStateDto>;
  playerStats: () => Promise<StatsDto>;
  playerOptions: () => Promise<PlayerOptionsDto>;
  setAudioTrack: (id: number) => Promise<PlayerOptionsDto>;
  /** `null` turns subtitles off. */
  setSubtitleTrack: (id: number | null) => Promise<PlayerOptionsDto>;
  setAspect: (aspect: AspectMode) => Promise<PlayerOptionsDto>;
  setDeinterlace: (mode: DeinterlaceMode) => Promise<PlayerOptionsDto>;
  thumbnailAt: (behindLive: number) => Promise<ThumbnailDto | null>;
  /** The open movie's or episode's preview for the slot nearest `at`. */
  thumbnailForTitle: (at: number) => Promise<TitleThumbnailDto | null>;
  account: () => Promise<AccountDto>;
  setAccount: (host: string, username: string, password: string) => Promise<void>;
  /** `null` keeps what is on file — the redacted playlist or guides. */
  setM3uAccount: (playlist: string | null, epgUrls: string[] | null, userAgent: string) => Promise<void>;
  /** `null` keeps the guide list on file, which was shown redacted. */
  setHdhomerunAccount: (device: string, epgUrls: string[] | null) => Promise<void>;
  /** An empty password keeps the one on file; an empty username is a server
   *  that needs no login. */
  setTvheadendAccount: (url: string, username: string, password: string) => Promise<void>;
  testAccount: () => Promise<string>;
  /** Tests an account as typed, without saving it or touching the one in
   *  use. */
  testAccountCandidate: (candidate: AccountCandidateDto) => Promise<string>;
  settings: () => Promise<SettingsDto>;
  setSettings: (settings: SettingsDto) => Promise<void>;
  setSubtitleStyle: (style: SubtitleStyleDto) => Promise<void>;
  applyPlaybackSettings: (settings: SettingsDto) => Promise<void>;
  /** Stops the reconnect the backend is making by itself after a network
   *  failure; mid-attempt, the attempt too. */
  cancelReconnect: () => Promise<void>;
  /** The account's connections: allowed, open, when it expires. */
  connectionStatus: () => Promise<ConnectionsDto>;
  /** What the desktop's media controls show; `null` with nothing on. */
  setMediaSession: (session: MediaSessionDto | null) => Promise<void>;
  guideRows: (streamIds: number[], from: number, to: number) => Promise<GuideRowDto[]>;
  favourites: (limit: number) => Promise<FavouriteDto[]>;
  recentlyWatched: (limit: number) => Promise<FavouriteDto[]>;
  forgetActivity: (streamId: number) => Promise<void>;
  moviesSummary: () => Promise<MoviesSummaryDto>;
  movieCategories: () => Promise<VodCategoryDto[]>;
  movies: (query: MovieQueryDto, offset: number, limit: number) => Promise<MoviePageDto>;
  /** Fetched from the provider the first time and kept for a week. */
  movieDetails: (movieId: number) => Promise<MovieDetailsDto>;
  similarMovies: (movieId: number, limit: number) => Promise<MovieDto[]>;
  /** An absent mark is left as it is. */
  setMovieMarks: (movieId: number, marks: {
      favourite?: boolean;
      watched?: boolean;
  }) => Promise<MarksDto>;
  showsSummary: () => Promise<ShowsSummaryDto>;
  seriesCategories: () => Promise<SeriesCategoryDto[]>;
  series: (query: SeriesQueryDto, offset: number, limit: number) => Promise<SeriesPageDto>;
  /** The show's details and episodes: fetched from the provider the first
   *  time it is opened and kept for a week. */
  seriesDetails: (seriesId: number) => Promise<SeriesDetailsDto>;
  /** An absent mark is left as it is. Watched reaches every episode. */
  setSeriesMarks: (seriesId: number, marks: {
      favourite?: boolean;
      watched?: boolean;
  }) => Promise<MarksDto>;
  setEpisodeMarks: (episodeId: number, watched: boolean) => Promise<MarksDto>;
  /** Continue watching, newest first: movies and episodes in progress and the
   *  next episode of shows being followed, one entry per movie and show. */
  continueWatching: (limit: number) => Promise<ContinueItemDto[]>;
  /** Takes a movie or a show out of Continue watching. */
  removeFromContinue: (kind: ContinueKind, id: number) => Promise<void>;
  /** A movie's marks and where it was left, as the catalogue has them now. */
  movieMarks: (movieId: number) => Promise<MarksDto>;
  /** An episode's marks and where it was left, as the catalogue has them now. */
  episodeMarks: (episodeId: number) => Promise<MarksDto>;
  playingEpisode: (episodeId: number) => Promise<PlayingEpisodeDto>;
  onlineServices: () => Promise<OnlineServicesDto>;
  /** An absent field is left as it is. */
  setOpenSubtitles: (patch: {
      enabled?: boolean;
      languages?: string[];
  }) => Promise<OnlineServicesDto>;
  signInOpenSubtitles: (username: string, password: string) => Promise<OnlineServicesDto>;
  signOutOpenSubtitles: () => Promise<OnlineServicesDto>;
  findSubtitles: (kind: OnDemandKind, id: number, languages: string[]) => Promise<SubtitleSearchDto>;
  downloadSubtitle: (kind: OnDemandKind, id: number, fileId: number) => Promise<SubtitleDownloadDto>;
  keptSubtitles: (kind: OnDemandKind, id: number) => Promise<KeptSubtitleDto[]>;
  /** Adds a kept subtitle to what is playing, and selects it. */
  addSubtitleFile: (kind: OnDemandKind, id: number, fileId: number, title: string, lang: string) => Promise<PlayerOptionsDto>;
  activityRows: () => Promise<ActivityRowDto[]>;
  importActivity: (rows: ActivityRowDto[]) => Promise<number>;
  /** Asks where to save; the file's name, or `null` if cancelled. */
  exportBackup: (document: unknown, includeAccount: boolean, fileName: string) => Promise<string | null>;
  /** Asks for a file and reads it; `null` if cancelled. */
  openBackup: () => Promise<BackupFileDto | null>;
  /** Writes the account from the backup opened last. */
  importBackupAccount: () => Promise<void>;
  updateStatus: () => Promise<UpdateStatusDto>;
  updateCheck: () => Promise<UpdateStatusDto>;
  updateDownload: () => Promise<UpdateStatusDto>;
  updateCancel: () => Promise<UpdateStatusDto>;
  updateInstall: (when?: InstallWhen) => Promise<InstallOutcomeDto>;
  updateRestart: () => Promise<void>;
  updateOpenPage: () => Promise<void>;
  /** Opens OpenSubtitles' sign-up page; the address is the backend's. */
  openOpenSubtitlesSignUp: () => Promise<void>;
  recordings: () => Promise<RecordingDto[]>;
  recordProgramme: (request: ScheduleRequest) => Promise<RecordingDto>;
  previewRecording: (request: ScheduleRequest) => Promise<ConflictDto[]>;
  cancelRecording: (id: number) => Promise<void>;
  recordingBackups: (id: number) => Promise<BackupsDto>;
  setRecordingBackup: (id: number, streamId: number | null) => Promise<RecordingDto>;
  deleteRecording: (id: number, files: boolean) => Promise<void>;
  recordingStatus: () => Promise<RecordingStatusDto>;
  recordingSettings: () => Promise<RecordingSettingsDto>;
  setRecordingSettings: (settings: RecordingSettingsDto) => Promise<RecordingSettingsDto>;
  recordingNetwork: () => Promise<NetworkDto>;
  setRecordingNetwork: (settings: NetworkSettingsDto) => Promise<NetworkDto>;
  setRecordingPassword: (password: string) => Promise<NetworkDto>;
  revokeRecordingDevice: (id: string) => Promise<NetworkDto>;
  recordingService: () => Promise<ServiceStatusDto>;
  installRecordingService: (linger: boolean) => Promise<ServiceStatusDto>;
  removeRecordingService: () => Promise<ServiceStatusDto>;
  setRecordingLinger: (on: boolean) => Promise<ServiceStatusDto>;
  openRecordingFolder: (id: number) => Promise<void>;
}
