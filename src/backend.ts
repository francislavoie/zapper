// One switch between the backends the interface can run on.
//
// Components import `backend`, never a backend module. Two backends:
//
//  - **web**: a browser served by the recording service (src/web.ts), talking
//    to its HTTP API on the same origin. Any build of the interface that is
//    not `vite dev` is this, since the recording service is the only thing
//    that serves it.
//  - **mock**: `vite dev` in a plain browser (src/mock.ts), so the UI can be
//    run and looked at without mpv or the Rust side.
//
// What each can do is `capabilities` (src/logic/capabilities.ts), which the
// components ask rather than which backend this is.

import type { Backend } from "./backendContract";
import { BROWSER, MOCK_DESKTOP, type Capabilities } from "./logic/capabilities";
import { passwordInClear } from "./logic/web";
import {
  mockBackend,
  onMockChannelQuality,
  onMockMediaKey,
  onMockPlayerOptions,
  onMockPlayerState,
  onMockSyncStatus,
  onMockUpdate,
} from "./mock";
import type {
  ChannelQualityDto,
  MediaKey,
  PlayerOptionsDto,
  PlayerStateDto,
  SyncStatusDto,
  UpdateStatusDto,
} from "./types";
import {
  onNothing,
  onWebPlayerOptions,
  onWebPlayerState,
  onWebSyncStatus,
  signOut as webSignOut,
  webBackend,
} from "./web";
import { playableHere } from "./webPlayer";

export type { Backend };

export type BackendKind = "web" | "mock";

/** `VITE_ZAPPER_BACKEND=web` runs `vite dev` against a recording service
 *  behind its proxy instead of the mock; nothing else sets it. */
const wanted = import.meta.env.VITE_ZAPPER_BACKEND as string | undefined;

export const backendKind: BackendKind =
  wanted === "web" || (wanted !== "mock" && !import.meta.env.DEV) ? "web" : "mock";

/** True only in the browser-driven mock. Guards the one piece of chrome that
 *  must never appear in the real app: a painted stand-in for the video plane. */
export const isMock = backendKind === "mock";

/** What the interface can do here. A browser without Media Source
 *  Extensions (an iPhone before iOS 17.1) cannot play. */
export const capabilities: Capabilities =
  backendKind === "web" ? { ...BROWSER, playback: playableHere } : MOCK_DESKTOP;

/** Whether a password typed here crosses the network unencrypted: a browser
 *  reaching the recording service over plain HTTP from another computer.
 *  The mock never crosses a network. */
export const passwordsInClear: boolean =
  backendKind === "web" &&
  typeof location !== "undefined" &&
  passwordInClear(location.protocol, location.hostname);

/** The mock matches the web backend; it is cast, as it always was, because
 *  it keeps a few extras for drivers. */
export const backend: Backend =
  backendKind === "web" ? webBackend : (mockBackend as unknown as Backend);

/** Signs this browser out of the recording service. Only offered where
 *  `capabilities.signOut`. */
export const signOut = (): Promise<void> =>
  backendKind === "web" ? webSignOut() : Promise.resolve();

type Unsub = () => void;

const pick = <T>(web: T, mock: T): T => (backendKind === "web" ? web : mock);

export const subscribeSync = (f: (s: SyncStatusDto) => void): Promise<Unsub> =>
  pick(onWebSyncStatus, onMockSyncStatus)(f);

export const subscribePlayer = (f: (s: PlayerStateDto) => void): Promise<Unsub> =>
  pick(onWebPlayerState, onMockPlayerState)(f);

export const subscribePlayerOptions = (f: (s: PlayerOptionsDto) => void): Promise<Unsub> =>
  pick(onWebPlayerOptions, onMockPlayerOptions)(f);

export const subscribeUpdate = (f: (s: UpdateStatusDto) => void): Promise<Unsub> =>
  pick(onNothing, onMockUpdate)(f);

export const subscribeMediaKey = (f: (k: MediaKey) => void): Promise<Unsub> =>
  pick(onNothing, onMockMediaKey)(f);

export const subscribeChannelQuality = (f: (q: ChannelQualityDto) => void): Promise<Unsub> =>
  pick(onNothing, onMockChannelQuality)(f);
