// One switch between the real command boundary and the mock.
//
// `src/api.ts` is the contract and must not be edited, so the choice is made
// here instead: components import `backend`, never `api`. Under Tauri this is
// `api` verbatim; under `vite dev` in a plain browser it is `src/mock.ts`, so
// the UI can be run and looked at without mpv or the Rust side.

import { api, onMediaKey, onPlayerOptions, onPlayerState, onSyncStatus, onUpdate } from "./api";
import {
  mockBackend,
  onMockMediaKey,
  onMockPlayerOptions,
  onMockPlayerState,
  onMockSyncStatus,
  onMockUpdate,
} from "./mock";
import type {
  MediaKey,
  PlayerOptionsDto,
  PlayerStateDto,
  SyncStatusDto,
  UpdateStatusDto,
} from "./types";

export const inTauri =
  typeof (globalThis as Record<string, unknown>).__TAURI_INTERNALS__ !== "undefined";

/** True only in the browser-driven mock. Guards the one piece of chrome that
 *  must never appear in the real app: a painted stand-in for the video plane. */
export const isMock = !inTauri;

/** Structurally identical to `api`; TypeScript checks that below. */
export const backend: typeof api = inTauri ? api : (mockBackend as unknown as typeof api);

type Unsub = () => void;

export const subscribeSync = (f: (s: SyncStatusDto) => void): Promise<Unsub> =>
  inTauri ? onSyncStatus(f) : onMockSyncStatus(f);

export const subscribePlayer = (f: (s: PlayerStateDto) => void): Promise<Unsub> =>
  inTauri ? onPlayerState(f) : onMockPlayerState(f);

export const subscribePlayerOptions = (f: (s: PlayerOptionsDto) => void): Promise<Unsub> =>
  inTauri ? onPlayerOptions(f) : onMockPlayerOptions(f);

export const subscribeUpdate = (f: (s: UpdateStatusDto) => void): Promise<Unsub> =>
  inTauri ? onUpdate(f) : onMockUpdate(f);

export const subscribeMediaKey = (f: (k: MediaKey) => void): Promise<Unsub> =>
  inTauri ? onMediaKey(f) : onMockMediaKey(f);
