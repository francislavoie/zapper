// What the toolbar says, as arithmetic on a SyncStatusDto.
//
// The bar has one line of text and it is never blank: either background work
// is happening and it is named, or nothing is and the bar says how stale the
// data is. Both answers are pure functions of the status and the clock, so
// both are checkable — see src/__checks__/toolbar.test.mjs.

import type { Capabilities } from "./capabilities";
import type { SyncPhase, SyncStatusDto } from "../types";

/** The one number the layout hangs off: the toolbar's height in CSS pixels.
 *  It is mirrored by `--toolbar-h` in styles.css, and the two are asserted
 *  equal by measurement in the browser rather than trusted. Everything below
 *  the bar — including the rect handed to mpv — is measured from the DOM, so
 *  this value is never added to a coordinate by hand. */
export const TOOLBAR_HEIGHT = 38;

/** The tone of the status line, which is all the view needs to decide colour:
 *  working, resting, or broken. */
export type StatusTone = "busy" | "idle" | "bad";

export interface StatusLine {
  tone: StatusTone;
  /** The sentence in the bar. Never empty. */
  text: string;
  /** Whatever the backend had to add — a count, a filename — or "". */
  detail: string;
  /** Whether the spinner turns. */
  spinning: boolean;
  /** The link beside a failure: try the sync again, or go and set up the
   *  account it was missing. `null` when nothing is wrong. */
  action: StatusAction | null;
}

/** What the link beside the status line does. */
export type StatusAction = "retry" | "account";

/** How the sync says it has no account to sync from: the start of the error
 *  `credentials::load_account` returns, which the sync publishes as its
 *  failure message. Tied to that text by src/__checks__/toolbar.test.mjs. */
export const NO_ACCOUNT_MESSAGE = "no account configured";

/** Whether a failure is the missing account rather than a sync that went
 *  wrong. Asking again cannot fix that one — only an account can. */
export function isNoAccount(message: string): boolean {
  return message.trim().toLowerCase().startsWith(NO_ACCOUNT_MESSAGE);
}

/** Each phase in the words a person would use, not the backend's. "epg" is a
 *  wire word; "guide" is what this app calls it everywhere else. */
const BUSY_TEXT: Partial<Record<SyncPhase, string>> = {
  categories: "Refreshing categories…",
  channels: "Refreshing channels…",
  epg: "Loading guide…",
  movies: "Refreshing movies…",
};

export const isBusy = (phase: SyncPhase): boolean => phase in BUSY_TEXT;

/** Whole units, largest that fits, because "4,812 seconds ago" is not an
 *  answer anybody wanted. Anything under a minute is "just now": the bar
 *  re-renders on a coarse clock and second-by-second counting would be a lie
 *  about its own precision. */
export function relativeTime(then: number, now: number): string {
  const s = Math.floor(now - then);
  // A clock that has gone backwards — a resync, a suspend — is not worth a
  // negative number in the chrome.
  if (s < 60) return "just now";
  const units: Array<[number, string]> = [
    [86400 * 7, "week"],
    [86400, "day"],
    [3600, "hour"],
    [60, "minute"],
  ];
  for (const [size, name] of units) {
    const n = Math.floor(s / size);
    if (n >= 1) return `${n} ${name}${n === 1 ? "" : "s"} ago`;
  }
  return "just now";
}

/** The resting line: how old the catalogue is, or that there is none yet. */
export function restingText(lastSynced: number | null, now: number): string {
  if (lastSynced == null) return "Not synced yet";
  return `Synced ${relativeTime(lastSynced, now)}`;
}

export function statusLine(sync: SyncStatusDto, now: number): StatusLine {
  const busy = BUSY_TEXT[sync.phase];
  if (busy) {
    return { tone: "busy", text: busy, detail: sync.message, spinning: true, action: null };
  }
  if (sync.phase === "failed" && isNoAccount(sync.message)) {
    // Said as a state rather than an error, with the way out beside it: a
    // first run is not a failure, and "retry" would only fail again.
    return { tone: "bad", text: "No account yet", detail: "", spinning: false, action: "account" };
  }
  if (sync.phase === "failed") {
    return {
      tone: "bad",
      // The backend's message is the whole explanation when it has one, so it
      // is promoted out of the detail slot rather than buried beside a generic
      // headline.
      text: sync.message || "Sync failed",
      detail: "",
      spinning: false,
      action: "retry",
    };
  }
  // At rest the title bar says only when the catalogue was synced: its size is
  // the rail's All channels count, and the space goes to the recording badge.
  return {
    tone: "idle",
    text: restingText(sync.lastSynced, now),
    detail: "",
    spinning: false,
    action: null,
  };
}

/** The maximise button's label and glyph. The window tells us which state it
 *  is in; this only decides how to say it. */
export function maximiseLabel(maximised: boolean): { label: string; glyph: string } {
  return maximised
    ? { label: "Restore", glyph: "❐" }
    : { label: "Maximise", glyph: "▢" };
}

/** The app's top-level sections, as the title bar's tabs. */
export type Section = "live" | "movies" | "shows";

export const SECTION_LABEL: Record<Section, string> = {
  live: "Live TV",
  movies: "Movies",
  shows: "Shows",
};

/** The tabs to draw: Live TV always, Movies and Shows only where the
 *  provider's on-demand catalogue is there to fill them. */
export function titleTabs(caps: Pick<Capabilities, "onDemand">): Section[] {
  return caps.onDemand ? ["live", "movies", "shows"] : ["live"];
}
