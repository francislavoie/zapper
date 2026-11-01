// The pure half of the web backend (src/web.ts): how its requests are
// addressed and dressed, how answers become messages, how often the sync's
// status is asked for, and which settings a browser keeps for itself.
//
// Checked in src/__checks__/web.test.mjs.

import type { SettingsDto, SyncStatusDto } from "../types";

/** The header the recording service wants on every request that changes
 *  something (src-tauri/src/recording/web.rs): a page on another site
 *  cannot add it. */
export const CSRF_HEADER = "X-Zapper";

type Param = string | number | null | undefined;

/** `path?a=1&b=x`, leaving out what is null or undefined (an absent
 *  category is "every channel"), each value encoded. */
export function apiUrl(path: string, params: Record<string, Param> = {}): string {
  const q = Object.entries(params)
    .filter((e): e is [string, string | number] => e[1] !== null && e[1] !== undefined)
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(String(v))}`)
    .join("&");
  return q ? `${path}?${q}` : path;
}

/** Stream ids as the service reads them: `1,2,3`. */
export const idList = (ids: readonly number[]): string => ids.join(",");

/** The init for a request. Every one is same-origin with its cookie; one that
 *  changes something carries the CSRF header, and a body goes as JSON. */
export function requestInit(method: string, body?: unknown): RequestInit {
  const headers: Record<string, string> = { Accept: "application/json" };
  if (method !== "GET" && method !== "HEAD") headers[CSRF_HEADER] = "1";
  if (body !== undefined) headers["Content-Type"] = "application/json";
  return {
    method,
    headers,
    credentials: "same-origin",
    // Nothing from here belongs in a cache: what is on changes by the
    // minute, and a recording list must be the service's own.
    cache: "no-store",
    ...(body !== undefined ? { body: JSON.stringify(body) } : {}),
  };
}

/** What a failed answer says: the service's `{"error": …}`, or its status. */
export function errorText(status: number, body: unknown): string {
  if (body && typeof body === "object" && "error" in body) {
    const e = (body as { error: unknown }).error;
    if (typeof e === "string" && e.trim() !== "") return e;
  }
  if (status === 0) return "The recording service could not be reached.";
  return `The recording service answered ${status}.`;
}

/** An answer that means this browser is not signed in (any more). */
export const signedOutStatus = (status: number): boolean => status === 401;

/** How long to wait before asking the sync's status again: often while it is
 *  working, so the toolbar follows it; rarely otherwise, to notice a refresh
 *  that someone else started. */
export function syncPollMs(s: SyncStatusDto | null): number {
  if (s && (s.phase === "categories" || s.phase === "channels" || s.phase === "epg")) {
    return 2_000;
  }
  return 15_000;
}

/** The settings a browser keeps for itself, in its own storage: how the
 *  guide looks here. Everything else in `SettingsDto` belongs to the app on
 *  the service's computer, and is left as the defaults. */
export const BROWSER_SETTINGS = ["guideHours", "timeFormat"] as const;

export type BrowserSettings = Partial<Pick<SettingsDto, (typeof BROWSER_SETTINGS)[number]>>;

export const BROWSER_SETTINGS_KEY = "zapper.web.settings";

/** `base` with what this browser keeps laid over it. Anything stored that is
 *  not one of [`BROWSER_SETTINGS`], or not of its type, is ignored. */
export function withBrowserSettings(base: SettingsDto, stored: unknown): SettingsDto {
  if (!stored || typeof stored !== "object") return base;
  const s = stored as Record<string, unknown>;
  const out = { ...base };
  if (typeof s.guideHours === "number" && Number.isFinite(s.guideHours)) {
    out.guideHours = s.guideHours;
  }
  if (s.timeFormat === "auto" || s.timeFormat === "12h" || s.timeFormat === "24h") {
    out.timeFormat = s.timeFormat;
  }
  return out;
}

/** The part of `s` this browser keeps. */
export function browserSettings(s: SettingsDto): BrowserSettings {
  return { guideHours: s.guideHours, timeFormat: s.timeFormat };
}

/** What a feature this page cannot offer says, when something asks anyway. */
export const NOT_HERE = "Not available in the browser.";
export const NO_PLAYBACK =
  "Watching in the browser isn’t available yet: open the channel in the zapper app, or record it.";
