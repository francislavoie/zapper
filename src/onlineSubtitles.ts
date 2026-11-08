// The subtitle finder's state: which title its dialog is open on, the
// subtitles kept for each title, and the online-services settings. An
// external store rather than a slice of store.tsx, because nothing else reads
// it and the dialog can be opened from the player and from a details pane.
//
// Plain `.ts` with explicit extensions so a node check can run it.

import { useCallback, useEffect, useRef, useSyncExternalStore } from "react";
import { failed, idle, loading, ready, errorMessage, type Async } from "./logic/async.ts";
import { servicesAfterDownload } from "./logic/onlineSubtitles.ts";
import type { KeptSubtitleDto, OnDemandKind, OnlineServicesDto, SubtitleDownloadDto } from "./types";

export type TitleRef = { kind: OnDemandKind; id: number };

/** Titles whose kept subtitles are remembered. */
export const KEPT_TITLES = 20;

// ------------------------------------------------------------ the dialog

let open: TitleRef | null = null;
/** What focus goes back to when the dialog closes. */
let opener: HTMLElement | null = null;
const dialogListeners = new Set<() => void>();
const notifyDialog = () => dialogListeners.forEach((f) => f());

/** `from` is the control it was opened from (the subtitles button), when
 *  there is one; else whatever has focus. */
export function openFindSubtitles(t: TitleRef, from?: HTMLElement | null): void {
  open = { kind: t.kind, id: t.id };
  opener =
    from ??
    (typeof document !== "undefined" ? (document.activeElement as HTMLElement | null) : null);
  notifyDialog();
}

export function closeFindSubtitles(): void {
  if (open === null) return;
  open = null;
  notifyDialog();
}

/** The control the dialog was opened from, as it was opened. */
export const findOpener = (): HTMLElement | null => opener;

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
  (await import("./backend")).backend.keptSubtitles(kind, id);
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
/** The read under way for each title, and which one is the latest. */
const pending = new Map<string, { ticket: number; promise: Promise<void> }>();
let tickets = 0;
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

/** Reads the title's kept subtitles, once unless it is asked again. A read
 *  already under way is joined; a forced one supersedes it, and the older
 *  answer is dropped if it lands later. */
export function loadKept(t: TitleRef, force = false): Promise<void> {
  const key = keptKey(t);
  const current = pending.get(key);
  if (!force) {
    if (kept.has(key)) return Promise.resolve();
    if (current) return current.promise;
  }
  const ticket = ++tickets;
  const promise = (async () => {
    try {
      const list = await fetchKept(t.kind, t.id);
      if (pending.get(key)?.ticket === ticket) remember(key, list);
    } catch {
      // A failed read leaves what was known; the dialog's own search reports
      // what is wrong with the service.
    } finally {
      if (pending.get(key)?.ticket === ticket) pending.delete(key);
    }
  })();
  pending.set(key, { ticket, promise });
  return promise;
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
  pending.clear();
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
/** Bumped by every answer put in directly, so a read begun before it can't
 *  overwrite it when it lands. */
let generation = 0;
const serviceListeners = new Set<() => void>();
const notifyServices = (next: Async<OnlineServicesDto>) => {
  services = next;
  serviceListeners.forEach((f) => f());
};

export type ServicesFetch = () => Promise<OnlineServicesDto>;
const defaultServices: ServicesFetch = async () =>
  (await import("./backend")).backend.onlineServices();
let fetchServices: ServicesFetch = defaultServices;

/** For checks: where the settings come from, and a clean start. */
export function setServicesSource(f?: ServicesFetch): void {
  fetchServices = f ?? defaultServices;
  generation++;
  services = idle();
}

export const currentServices = (): Async<OnlineServicesDto> => services;

export function setServices(next: OnlineServicesDto): void {
  generation++;
  notifyServices(ready(next));
}

/** A download answered: the settings' account line follows what it said
 *  (the downloads left, a sign-in refused). Nothing when they were never
 *  read: their first read says the same. */
export function downloadAnswered(d: SubtitleDownloadDto): void {
  if (services.status !== "ready") return;
  const next = servicesAfterDownload(services.data, d);
  if (next !== services.data) setServices(next);
}

export async function loadServices(): Promise<void> {
  if (services.status === "loading") return;
  const mine = generation;
  if (services.status !== "ready") notifyServices(loading());
  try {
    const next = await fetchServices();
    if (generation === mine) notifyServices(ready(next));
  } catch (e) {
    if (generation === mine) notifyServices(failed(errorMessage(e)));
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
  const set = useCallback((next: OnlineServicesDto) => setServices(next), []);
  return { status, reload, set };
}
