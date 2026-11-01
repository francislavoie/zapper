// One switch between the backends the interface can run on.
//
// `src/api.ts` is the contract and must not be edited, so the choice is made
// here instead: components import `backend`, never `api`. Three backends:
//
//  - **tauri**: the app's own window. `api` verbatim.
//  - **web**: a browser served by the recording service (src/web.ts), talking
//    to its HTTP API on the same origin. Any build of the interface that is
//    not in Tauri and not `vite dev` is this, since the recording service is
//    the only other thing that serves it.
//  - **mock**: `vite dev` in a plain browser (src/mock.ts), so the UI can be
//    run and looked at without mpv or the Rust side.
//
// What each can do is `capabilities` (src/logic/capabilities.ts), which the
// components ask rather than which backend this is.

import {
  api,
  onChannelQuality,
  onMediaKey,
  onMenu,
  onPlayerOptions,
  onPlayerState,
  onSyncStatus,
  onUpdate,
} from "./api";
import { BROWSER, DESKTOP, type Capabilities } from "./logic/capabilities";
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
  MenuCommand,
  PlayerOptionsDto,
  PlayerStateDto,
  SyncStatusDto,
  UpdateStatusDto,
} from "./types";
import { onNothing, onWebSyncStatus, signOut as webSignOut, webBackend } from "./web";

export const inTauri =
  typeof (globalThis as Record<string, unknown>).__TAURI_INTERNALS__ !== "undefined";

export type BackendKind = "tauri" | "web" | "mock";

/** `VITE_ZAPPER_BACKEND=web` runs `vite dev` against a recording service
 *  behind its proxy instead of the mock; nothing else sets it. */
const wanted = import.meta.env.VITE_ZAPPER_BACKEND as string | undefined;

export const backendKind: BackendKind = inTauri
  ? "tauri"
  : wanted === "web" || (wanted !== "mock" && !import.meta.env.DEV)
    ? "web"
    : "mock";

/** True only in the browser-driven mock. Guards the one piece of chrome that
 *  must never appear in the real app: a painted stand-in for the video plane. */
export const isMock = backendKind === "mock";

/** What the interface can do here. */
export const capabilities: Capabilities = backendKind === "web" ? BROWSER : DESKTOP;

/** Whether a password typed here crosses the network unencrypted: a browser
 *  reaching the recording service over plain HTTP from another computer.
 *  The app's window talks to its own Rust side, and never does. */
export const passwordsInClear: boolean =
  backendKind === "web" &&
  typeof location !== "undefined" &&
  passwordInClear(location.protocol, location.hostname);

/** Structurally identical to `api`; TypeScript checks that below. */
export const backend: typeof api =
  backendKind === "tauri"
    ? api
    : backendKind === "web"
      ? webBackend
      : (mockBackend as unknown as typeof api);

/** Signs this browser out of the recording service. Only offered where
 *  `capabilities.signOut`. */
export const signOut = (): Promise<void> =>
  backendKind === "web" ? webSignOut() : Promise.resolve();

type Unsub = () => void;

const pick = <T>(tauri: T, web: T, mock: T): T =>
  backendKind === "tauri" ? tauri : backendKind === "web" ? web : mock;

export const subscribeSync = (f: (s: SyncStatusDto) => void): Promise<Unsub> =>
  pick(onSyncStatus, onWebSyncStatus, onMockSyncStatus)(f);

export const subscribePlayer = (f: (s: PlayerStateDto) => void): Promise<Unsub> =>
  pick(onPlayerState, onNothing, onMockPlayerState)(f);

export const subscribePlayerOptions = (f: (s: PlayerOptionsDto) => void): Promise<Unsub> =>
  pick(onPlayerOptions, onNothing, onMockPlayerOptions)(f);

export const subscribeUpdate = (f: (s: UpdateStatusDto) => void): Promise<Unsub> =>
  pick(onUpdate, onNothing, onMockUpdate)(f);

export const subscribeMediaKey = (f: (k: MediaKey) => void): Promise<Unsub> =>
  pick(onMediaKey, onNothing, onMockMediaKey)(f);

/** The macOS app menu's Settings… and About. A browser has no app menu, so
 *  the mock never sends one. */
export const subscribeMenu = (f: (c: MenuCommand) => void): Promise<Unsub> =>
  inTauri ? onMenu(f) : Promise.resolve(() => {});

export const subscribeChannelQuality = (f: (q: ChannelQualityDto) => void): Promise<Unsub> =>
  pick(onChannelQuality, onNothing, onMockChannelQuality)(f);
