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
// What has no meaning here — the player, the window, the updater, the
// account, installing the service — is hidden through `BROWSER`'s
// capabilities. The calls are still answered, harmlessly: player controls do
// nothing with nothing playing, and the rest say they are not available.

import type { api } from "./api";
import { DEFAULT_SETTINGS } from "./logic/settings";
import {
  BROWSER_SETTINGS_KEY,
  NO_PLAYBACK,
  NOT_HERE,
  apiUrl,
  browserSettings,
  errorText,
  idList,
  requestInit,
  signedOutStatus,
  syncPollMs,
  withBrowserSettings,
} from "./logic/web";
import { initialPlayer } from "./store";
import type {
  CategoryDto,
  ChannelDto,
  DeviceDto,
  FavouriteDto,
  GuideRowDto,
  NetworkDto,
  PlayerOptionsDto,
  ProgrammeDto,
  ProgrammeHitDto,
  RecordingDto,
  RecordingSettingsDto,
  RecordingStatusDto,
  ScheduleRequest,
  SettingsDto,
  SyncStatusDto,
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

// ------------------------------------------------------------------ backend

const noOptions: PlayerOptionsDto = {
  audio: [],
  subtitles: [],
  aspect: "auto",
  deinterlace: "auto",
};

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
  syncNow: () => post<unknown>("/api/sync").then(() => undefined),

  // Nothing plays here yet, so there is nothing to control.
  play: () => Promise.reject(new Error(NO_PLAYBACK)),
  playCatchup: () => Promise.reject(new Error(NO_PLAYBACK)),
  stop: nothing,
  togglePause: nothing,
  seekRelative: nothing,
  seekLive: nothing,
  setVolume: nothing,
  setMuted: nothing,
  setCompressor: nothing,
  setVideoRect: nothing,
  playerState: () => Promise.resolve(initialPlayer),
  playerStats: notHere,
  playerOptions: () => Promise.resolve(noOptions),
  setAudioTrack: () => Promise.resolve(noOptions),
  setSubtitleTrack: () => Promise.resolve(noOptions),
  setAspect: () => Promise.resolve(noOptions),
  setDeinterlace: () => Promise.resolve(noOptions),
  thumbnailAt: () => Promise.resolve(null),
  cancelReconnect: nothing,
  setMediaSession: nothing,
  applyPlaybackSettings: nothing,
  setSubtitleStyle: nothing,

  // The provider account is the app's, on the service's computer.
  account: notHere,
  setAccount: notHere,
  setM3uAccount: notHere,
  setHdhomerunAccount: notHere,
  setTvheadendAccount: notHere,
  testAccount: notHere,
  testAccountCandidate: notHere,
  connectionStatus: notHere,

  // The app's settings are the app's; a browser keeps how its guide looks.
  settings: () => Promise.resolve(withBrowserSettings(DEFAULT_SETTINGS, storedSettings())),
  setSettings: (s) => {
    keepSettings(s);
    return Promise.resolve();
  },

  favourites: (limit) => get<FavouriteDto[]>(apiUrl("/api/favourites", { limit })),
  forgetActivity: (streamId) =>
    del<unknown>(`/api/favourites/${streamId}`).then(() => undefined),

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
  cancelRecording: (id) => post<unknown>(`/api/recordings/${id}/cancel`).then(() => undefined),
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
  const tick = async () => {
    try {
      last = await webBackend.syncStatus();
      if (live) f(last);
    } catch {
      // Signed out, or the service is restarting: the gate deals with the
      // first, and the next poll with the second.
    }
    if (live) timer = setTimeout(() => void tick(), syncPollMs(last));
  };
  timer = setTimeout(() => void tick(), syncPollMs(null));
  return Promise.resolve(() => {
    live = false;
    if (timer !== undefined) clearTimeout(timer);
  });
}

/** For pushes a browser never gets: the player's, the updater's, the media
 *  keys'. */
export const onNothing = (): Promise<Unsub> => Promise.resolve(() => {});
