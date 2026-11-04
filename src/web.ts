// The interface's backend in a browser, served by the recording service:
// the same calls as src/api.ts makes through Tauri, made to the service's
// HTTP API on the same origin instead (docs/recording.md, "Web access").
//
// Signed in, the browser holds a cookie its scripts cannot read; this file
// never sees a token. Every request that changes something carries the
// header the service wants as proof it came from this page. An answer of 401
// means the sign-in is gone (revoked in the app, or signed out elsewhere),
// and the page goes back to the sign-in (src/components/WebGate.tsx).
//
// Watching plays in a <video> element through the service's own stream
// (src/webPlayer.ts; a movie's, src/webVod.ts). What has no meaning here —
// the window, the updater, backups, installing the service, mpv's own
// settings and statistics, the provider's archive — is hidden through
// `BROWSER`'s capabilities. The calls
// are still answered, harmlessly: they say they are not available.

import type { api } from "./api";
import { DEFAULT_SETTINGS } from "./logic/settings";
import {
  BROWSER_SETTINGS_KEY,
  NOT_HERE,
  SYNC_BURST_MS,
  apiUrl,
  browserSettings,
  errorText,
  idList,
  requestInit,
  signedOutStatus,
  syncPollMs,
  withBrowserSettings,
} from "./logic/web";
import * as player from "./webPlayer";
import * as vod from "./webVod";
import type {
  AccountCandidateDto,
  AccountDto,
  CategoryDto,
  ChannelDto,
  DeviceDto,
  FavouriteDto,
  GuideRowDto,
  MarksDto,
  MovieDetailsDto,
  MovieDto,
  MoviePageDto,
  MoviesSummaryDto,
  NetworkDto,
  PlayerOptionsDto,
  PlayerStateDto,
  ProgrammeDto,
  ProgrammeHitDto,
  BackupsDto,
  RecordingDto,
  ConflictDto,
  RecordingSettingsDto,
  RecordingStatusDto,
  ScheduleRequest,
  SettingsDto,
  SyncStatusDto,
  VodCategoryDto,
} from "./types";

// ------------------------------------------------------------- signing out

const signedOutListeners = new Set<() => void>();

/** Called whenever the service says this browser is not signed in. */
export function onSignedOut(f: () => void): () => void {
  signedOutListeners.add(f);
  return () => signedOutListeners.delete(f);
}

function signedOut() {
  for (const f of signedOutListeners) f();
}

// ---------------------------------------------------------------- requests

export class ServiceError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}

async function request<T>(method: string, url: string, body?: unknown): Promise<T> {
  let resp: Response;
  try {
    resp = await fetch(url, requestInit(method, body));
  } catch {
    throw new ServiceError(errorText(0, null), 0);
  }
  const text = await resp.text();
  let json: unknown = null;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    // Not JSON: a proxy's page, say. The status says enough.
  }
  if (!resp.ok) {
    if (signedOutStatus(resp.status)) signedOut();
    throw new ServiceError(errorText(resp.status, json), resp.status);
  }
  return json as T;
}

const get = <T>(url: string) => request<T>("GET", url);
const post = <T>(url: string, body?: unknown) => request<T>("POST", url, body);
const put = <T>(url: string, body?: unknown) => request<T>("PUT", url, body);
const del = <T>(url: string) => request<T>("DELETE", url);

const notHere = (): Promise<never> => Promise.reject(new Error(NOT_HERE));
const nothing = (): Promise<void> => Promise.resolve();

// ----------------------------------------------------------------- session

/** Who this browser is signed in as, or `null` when it is not. */
export async function session(): Promise<DeviceDto | null> {
  const s = await get<{ signedIn?: boolean; device?: DeviceDto }>("/api/session");
  return s.signedIn && s.device ? s.device : null;
}

/** Signs in with the service's password; the cookie is the service's to set. */
export async function signIn(password: string): Promise<DeviceDto> {
  const out = await request<{ device: DeviceDto }>("POST", "/api/login", {
    password,
    cookie: true,
  });
  return out.device;
}

/** Signs this browser out: the service forgets its device and its cookie. */
export async function signOut(): Promise<void> {
  try {
    await post("/api/logout");
  } finally {
    signedOut();
  }
}

// ---------------------------------------------------------- local settings

function storedSettings(): unknown {
  try {
    const raw = localStorage.getItem(BROWSER_SETTINGS_KEY);
    return raw ? (JSON.parse(raw) as unknown) : null;
  } catch {
    return null;
  }
}

function keepSettings(s: SettingsDto) {
  try {
    localStorage.setItem(BROWSER_SETTINGS_KEY, JSON.stringify(browserSettings(s)));
  } catch {
    // Storage refused (private mode): the change lasts until the page goes.
  }
}

// ------------------------------------------------------------------- sync

/** When the sync's status stops being asked for often (`syncPollMs`). */
let burstUntil = 0;
/** Asks the status now, if anything is following it. */
let pollNow: (() => void) | null = null;

/** After asking the service for a refresh: follow its status closely for a
 *  while, so the toolbar shows the pass starting and how it goes, and the
 *  lists read the catalogue again once it lands (catalogueRefresh.ts). */
function watchSyncStart(): void {
  burstUntil = Date.now() + SYNC_BURST_MS;
  pollNow?.();
}

async function saveAccount(form: AccountCandidateDto): Promise<void> {
  await put<AccountDto>("/api/account", form);
  watchSyncStart();
}

// ------------------------------------------------------------------ backend

player.onPlayerSignedOut(signedOut);

export const webBackend: typeof api = {
  categories: () => get<CategoryDto[]>("/api/categories"),
  channels: (categoryId, offset, limit) =>
    get<ChannelDto[]>(apiUrl("/api/channels", { category: categoryId, offset, limit })),
  channelsByIds: (ids) =>
    ids.length === 0
      ? Promise.resolve([])
      : get<ChannelDto[]>(apiUrl("/api/channels/by-id", { ids: idList(ids) })),
  channelNumbers: () => get<[string, number][]>("/api/channels/numbers"),
  search: (query, limit) =>
    get<ChannelDto[]>(apiUrl("/api/channels/search", { q: query, limit })),
  searchProgrammes: (query, offset, limit) =>
    get<ProgrammeHitDto[]>(apiUrl("/api/programmes/search", { q: query, offset, limit })),
  channelIndex: (categoryId, streamId) =>
    get<number | null>(apiUrl(`/api/channels/${streamId}/index`, { category: categoryId })),
  guide: (streamIds, from, to) =>
    get<[number, ProgrammeDto[]][]>(apiUrl("/api/guide", { ids: idList(streamIds), from, to })),
  guideRows: (streamIds, from, to) =>
    get<GuideRowDto[]>(apiUrl("/api/guide/rows", { ids: idList(streamIds), from, to })),

  syncStatus: () => get<SyncStatusDto>("/api/sync"),
  syncNow: () => post<unknown>("/api/sync").then(watchSyncStart),

  // The player is a <video> element here (src/webPlayer.ts), a movie's
  // controls its own (src/webVod.ts) while one is open. The provider's
  // archive, mpv's statistics and deinterlacing are not; subtitles are a
  // movie's.
  play: (streamId) => player.play(streamId, vod.handOver()),
  playCatchup: notHere,
  stop: async () => {
    await vod.stop();
    await player.stop();
  },
  togglePause: () => (vod.active() ? vod.togglePause() : player.togglePause()),
  seekRelative: (seconds) =>
    vod.active() ? vod.seekRelative(seconds) : player.seekRelative(seconds),
  seekLive: () => (vod.active() ? Promise.resolve() : player.seekLive()),
  setVolume: (volume) => player.setVolume(volume),
  setMuted: (muted) => player.setMuted(muted),
  setCompressor: (enabled) => player.setCompressor(enabled),
  // Placed by the page itself (App.tsx paints the element).
  setVideoRect: () => {
    player.applyAspect();
    return Promise.resolve();
  },
  playerState: () => Promise.resolve(player.state()),
  playerStats: notHere,
  playerOptions: () => Promise.resolve(vod.active() ? vod.options() : player.options()),
  setAudioTrack: (id) => (vod.active() ? vod.setAudioTrack(id) : player.setAudioTrack(id)),
  setSubtitleTrack: (id) =>
    vod.active() ? vod.setSubtitleTrack(id) : Promise.resolve(player.options()),
  setAspect: (mode) => player.setAspect(mode),
  setDeinterlace: () => Promise.resolve(player.options()),
  thumbnailAt: () => Promise.resolve(null),
  cancelReconnect: nothing,
  setMediaSession: nothing,
  applyPlaybackSettings: nothing,
  setSubtitleStyle: nothing,

  // The provider account, kept by the service (and shared with the app on
  // its computer). Never sent with its secrets; a blank password keeps the
  // one on file, as in the app. Saving starts a refresh there.
  account: () => get<AccountDto>("/api/account"),
  setAccount: (host, username, password) =>
    saveAccount({ kind: "xtream", host, username, password }),
  setM3uAccount: (playlist, epgUrls, userAgent) =>
    saveAccount({ kind: "m3u", playlist, epgUrls, userAgent }),
  setHdhomerunAccount: (device, epgUrls) => saveAccount({ kind: "hdhomerun", device, epgUrls }),
  setTvheadendAccount: (url, username, password) =>
    saveAccount({ kind: "tvheadend", url, username, password }),
  testAccount: () => post<{ result: string }>("/api/account/test").then((r) => r.result),
  testAccountCandidate: (candidate) =>
    post<{ result: string }>("/api/account/test", candidate).then((r) => r.result),
  // How many streams the account has open is about playing here.
  connectionStatus: notHere,

  // The app's settings are the app's; a browser keeps how its guide looks.
  settings: () => Promise.resolve(withBrowserSettings(DEFAULT_SETTINGS, storedSettings())),
  setSettings: (s) => {
    keepSettings(s);
    return Promise.resolve();
  },

  favourites: (limit) => get<FavouriteDto[]>(apiUrl("/api/favourites", { limit })),
  recentlyWatched: (limit) => get<FavouriteDto[]>(apiUrl("/api/recent", { limit })),
  forgetActivity: (streamId) =>
    del<unknown>(`/api/favourites/${streamId}`).then(() => undefined),

  playMovie: (movieId, start) => vod.playMovie(movieId, start),
  seekTo: (seconds) => vod.seekTo(seconds),
  moviesSummary: () => get<MoviesSummaryDto>("/api/vod/summary"),
  movieCategories: () => get<VodCategoryDto[]>("/api/vod/categories"),
  movies: (query, offset, limit) =>
    get<MoviePageDto>(
      apiUrl("/api/vod/movies", {
        list: query.list,
        category: query.categoryId,
        sort: query.sort,
        from: query.yearFrom,
        to: query.yearTo,
        offset,
        limit,
      }),
    ),
  searchMovies: (query, limit) =>
    get<MovieDto[]>(apiUrl("/api/vod/movies/search", { q: query, limit })),
  movieDetails: (movieId) => get<MovieDetailsDto>(`/api/vod/movies/${movieId}`),
  similarMovies: (movieId, limit) =>
    get<MovieDto[]>(apiUrl(`/api/vod/movies/${movieId}/similar`, { limit })),
  setMovieMarks: (movieId, marks) =>
    put<MarksDto>(`/api/vod/movies/${movieId}/marks`, marks),

  activityRows: notHere,
  importActivity: notHere,
  exportBackup: notHere,
  openBackup: notHere,
  importBackupAccount: notHere,

  updateStatus: notHere,
  updateCheck: notHere,
  updateDownload: notHere,
  updateCancel: notHere,
  updateInstall: notHere,
  updateRestart: notHere,
  updateOpenPage: notHere,

  recordings: () => get<RecordingDto[]>("/api/recordings"),
  recordProgramme: (req: ScheduleRequest) => post<RecordingDto>("/api/recordings", req),
  // A service too old to say has no conflicts to tell.
  previewRecording: (req: ScheduleRequest) =>
    post<ConflictDto[]>("/api/recordings/preview", req).catch(() => []),
  cancelRecording: (id) => post<unknown>(`/api/recordings/${id}/cancel`).then(() => undefined),
  recordingBackups: (id) => get<BackupsDto>(`/api/recordings/${id}/backups`),
  setRecordingBackup: (id, streamId) =>
    put<RecordingDto>(`/api/recordings/${id}/backup`, { streamId }),
  deleteRecording: (id, files) =>
    del<unknown>(apiUrl(`/api/recordings/${id}`, { files: files ? "true" : "false" })).then(
      () => undefined,
    ),
  recordingStatus: () => get<RecordingStatusDto>("/api/status"),
  recordingSettings: () => get<RecordingSettingsDto>("/api/settings"),
  setRecordingSettings: (s) => put<RecordingSettingsDto>("/api/settings", s),
  // The service answers these to the app alone; asked from here, it says so.
  recordingNetwork: () => get<NetworkDto>("/api/network"),
  setRecordingNetwork: (s) => put<NetworkDto>("/api/network", s),
  setRecordingPassword: (password) => put<NetworkDto>("/api/network/password", { password }),
  revokeRecordingDevice: (id) => del<NetworkDto>(`/api/devices/${encodeURIComponent(id)}`),
  recordingService: notHere,
  installRecordingService: notHere,
  removeRecordingService: notHere,
  setRecordingLinger: notHere,
  openRecordingFolder: notHere,
};

type Unsub = () => void;

/** The sync's status, asked for again and again ([`syncPollMs`]): the service
 *  pushes nothing to a browser. */
export function onWebSyncStatus(f: (s: SyncStatusDto) => void): Promise<Unsub> {
  let live = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let last: SyncStatusDto | null = null;
  // One chain of polls: asked for now while a poll is out, the next one
  // follows it at once rather than starting a second chain beside it.
  let asking = false;
  let again = false;
  const tick = async () => {
    asking = true;
    try {
      last = await webBackend.syncStatus();
      if (live) f(last);
    } catch {
      // Signed out, or the service is restarting: the gate deals with the
      // first, and the next poll with the second.
    }
    asking = false;
    if (!live) return;
    const wait = again ? 0 : syncPollMs(last, Date.now(), burstUntil);
    again = false;
    timer = setTimeout(() => void tick(), wait);
  };
  const now = () => {
    if (asking) {
      again = true;
      return;
    }
    if (timer !== undefined) clearTimeout(timer);
    void tick();
  };
  pollNow = now;
  timer = setTimeout(() => void tick(), syncPollMs(null));
  return Promise.resolve(() => {
    live = false;
    if (pollNow === now) pollNow = null;
    if (timer !== undefined) clearTimeout(timer);
  });
}

/** The player's state and options, as the <video> element changes. */
export const onWebPlayerState = (f: (s: PlayerStateDto) => void): Promise<Unsub> =>
  Promise.resolve(player.playerFeed.on(f));
export const onWebPlayerOptions = (f: (s: PlayerOptionsDto) => void): Promise<Unsub> =>
  Promise.resolve(player.optionsFeed.on(f));

/** For pushes a browser never gets: the updater's, the media keys', a
 *  channel's measured quality. */
export const onNothing = (): Promise<Unsub> => Promise.resolve(() => {});
