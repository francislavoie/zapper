// The subtitle finder's state: which title its dialog is open on, the
// subtitles kept for each title, and the online-services settings. An
// external store rather than a slice of store.tsx, because nothing else reads
// it and the dialog can be opened from the player and from a details pane.
//
// Plain `.ts` with explicit extensions so a node check can run it.

import { useCallback, useEffect, useRef, useSyncExternalStore } from "react";
import { failed, idle, loading, ready, errorMessage, type Async } from "./logic/async.ts";
import type { KeptSubtitleDto, OnDemandKind, OnlineServicesDto } from "./types";

export type TitleRef = { kind: OnDemandKind; id: number };

/** Titles whose kept subtitles are remembered. */
export const KEPT_TITLES = 20;

// ------------------------------------------------------------ the dialog

let open: TitleRef | null = null;
const dialogListeners = new Set<() => void>();
const notifyDialog = () => dialogListeners.forEach((f) => f());

export function openFindSubtitles(t: TitleRef): void {
  open = { kind: t.kind, id: t.id };
  notifyDialog();
}

export function closeFindSubtitles(): void {
  if (open === null) return;
  open = null;
  notifyDialog();
}

/** The title the dialog is open on, outside a component. */
export const currentFind = (): TitleRef | null => open;

export function useFindSubtitles(): TitleRef | null {
  return useSyncExternalStore(
    (f) => {
      dialogListeners.add(f);
      return () => dialogListeners.delete(f);
    },
    () => open,
    () => null,
  );
}

// ------------------------------------------------------------------ kept

export type KeptFetch = (kind: OnDemandKind, id: number) => Promise<KeptSubtitleDto[]>;

const defaultFetch: KeptFetch = async (kind, id) =>
  (await import("./api")).api.keptSubtitles(kind, id);
let fetchKept: KeptFetch = defaultFetch;

/** For checks: where kept subtitles come from. No argument restores the backend. */
export function setKeptSource(f?: KeptFetch): void {
  fetchKept = f ?? defaultFetch;
}

export const keptKey = (t: TitleRef): string => `${t.kind}:${t.id}`;

// Insertion order is the order of use: a Map's first key is the oldest.
const kept = new Map<string, KeptSubtitleDto[]>();
const keptListeners = new Set<() => void>();
let keptVersion = 0;
const inflight = new Map<string, number>();
const notifyKept = () => {
  keptVersion++;
  keptListeners.forEach((f) => f());
};
const NONE: KeptSubtitleDto[] = [];

function remember(key: string, list: KeptSubtitleDto[]) {
  kept.delete(key);
  kept.set(key, list);
  while (kept.size > KEPT_TITLES) kept.delete(kept.keys().next().value as string);
  notifyKept();
}

/** Reads the title's kept subtitles, once unless it is asked again. */
export async function loadKept(t: TitleRef, force = false): Promise<void> {
  const key = keptKey(t);
  if (!force && kept.has(key)) return;
  const ticket = (inflight.get(key) ?? 0) + 1;
  inflight.set(key, ticket);
  try {
    const list = await fetchKept(t.kind, t.id);
    if (inflight.get(key) === ticket) remember(key, list);
  } catch {
    // A failed read leaves what was known; the dialog's own search reports
    // what is wrong with the service.
  }
}

/** The cached list for a title, without fetching. */
export const keptFor = (t: TitleRef): KeptSubtitleDto[] | undefined => kept.get(keptKey(t));

/** A subtitle was downloaded: read the list again. */
export function keptChanged(t: TitleRef): void {
  void loadKept(t, true);
}

/** For checks. */
export function clearKept(): void {
  kept.clear();
  inflight.clear();
  notifyKept();
}

export function useKeptSubtitles(t: TitleRef | null): KeptSubtitleDto[] {
  useSyncExternalStore(
    (f) => {
      keptListeners.add(f);
      return () => keptListeners.delete(f);
    },
    () => keptVersion,
    () => 0,
  );
  const key = t ? keptKey(t) : null;
  useEffect(() => {
    if (t) void loadKept(t);
    // Keyed by the title's value, not its object.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  return (key && kept.get(key)) || NONE;
}

// ------------------------------------------------------ online services

let services: Async<OnlineServicesDto> = idle();
const serviceListeners = new Set<() => void>();
const notifyServices = (next: Async<OnlineServicesDto>) => {
  services = next;
  serviceListeners.forEach((f) => f());
};

async function loadServices() {
  if (services.status === "loading") return;
  if (services.status !== "ready") notifyServices(loading());
  try {
    notifyServices(ready(await (await import("./api")).api.onlineServices()));
  } catch (e) {
    notifyServices(failed(errorMessage(e)));
  }
}

export function useOnlineServices(): {
  status: Async<OnlineServicesDto>;
  reload(): void;
  set(next: OnlineServicesDto): void;
} {
  const status = useSyncExternalStore(
    (f) => {
      serviceListeners.add(f);
      return () => serviceListeners.delete(f);
    },
    () => services,
    () => services,
  );
  const started = useRef(false);
  useEffect(() => {
    if (!started.current && services.status === "idle") {
      started.current = true;
      void loadServices();
    }
  }, []);
  const reload = useCallback(() => void loadServices(), []);
  const set = useCallback((next: OnlineServicesDto) => notifyServices(ready(next)), []);
  return { status, reload, set };
}
