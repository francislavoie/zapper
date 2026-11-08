// The pure half of the web backend (src/web.ts): how its requests are
// addressed and dressed, how answers become messages, how often the sync's
// status is asked for, and which settings a browser keeps for itself.
//
// Checked in src/__checks__/web.test.mjs.

import { isBusy } from "./toolbar.ts";
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

/** How long after asking for a refresh (or saving the account, which asks
 *  for one) the status is asked for often even though it still says idle:
 *  the service starts the pass when its loop wakes, not before it answers. */
export const SYNC_BURST_MS = 20_000;

/** How long to wait before asking the sync's status again: often while it is
 *  working, so the toolbar follows it, and just after this page asked for a
 *  refresh, so the toolbar shows it starting; rarely otherwise, to notice a
 *  refresh that someone else started. `burstUntil` is when the "just after"
 *  ends, on the clock `now` is read from. */
export function syncPollMs(s: SyncStatusDto | null, now = 0, burstUntil = 0): number {
  if (s && (isBusy(s.phase))) {
    return 2_000;
  }
  if (now < burstUntil) return 1_000;
  return 15_000;
}

/** Whether a password typed on this page would cross the network
 *  unencrypted: plain HTTP to anything but this computer itself. */
export function passwordInClear(protocol: string, hostname: string): boolean {
  if (protocol !== "http:") return false;
  const host = hostname.toLowerCase().replace(/^\[|\]$/g, "");
  return !(host === "localhost" || host === "::1" || /^127\.\d+\.\d+\.\d+$/.test(host));
}

/** The settings a browser keeps for itself, in its own storage: how the
 *  guide looks here, and whether subtitles are on and in what language.
 *  Everything else in `SettingsDto` belongs to the app on the service's
 *  computer, and is left as the defaults. */
export const BROWSER_SETTINGS = ["guideHours", "timeFormat", "subtitlesOn", "subtitleLanguage"] as const;

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
  if (typeof s.subtitlesOn === "boolean") out.subtitlesOn = s.subtitlesOn;
  if (s.subtitleLanguage === null || typeof s.subtitleLanguage === "string") {
    out.subtitleLanguage = s.subtitleLanguage;
  }
  return out;
}

/** The part of `s` this browser keeps. */
export function browserSettings(s: SettingsDto): BrowserSettings {
  return {
    guideHours: s.guideHours,
    timeFormat: s.timeFormat,
    subtitlesOn: s.subtitlesOn,
    subtitleLanguage: s.subtitleLanguage,
  };
}

/** What a feature this page cannot offer says, when something asks anyway. */
export const NOT_HERE = "Not available in the browser.";
/** What choosing a channel says in a browser that can't play live TV. */
export { UNSUPPORTED as NO_PLAYBACK } from "./webPlayback.ts";
