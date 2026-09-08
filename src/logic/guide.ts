// Guide arithmetic: time to pixels, and which programme is on.
//
// Everything here is pure so the grid's geometry can be checked without a
// browser — see src/__checks__/guide.test.mjs.

import type { ProgrammeDto } from "../types";

export const HOUR = 3600;

/** Which of now / next a programme list is offering, relative to a clock. */
export interface NowNext {
  now: ProgrammeDto | null;
  next: ProgrammeDto | null;
}

/** The programme on air at `nowSeconds`, and the one after it.
 *
 *  The list is not assumed sorted: XMLTV can arrive in whatever
 *  order the file had, and one out-of-order entry should not silently blank
 *  the panel. */
export function splitNowNext(
  programmes: readonly ProgrammeDto[],
  nowSeconds: number,
): NowNext {
  let live: ProgrammeDto | null = null;
  let next: ProgrammeDto | null = null;
  for (const p of programmes) {
    if (p.start <= nowSeconds && nowSeconds < p.stop) {
      // Ties go to the one that started later: overlapping XMLTV entries are
      // usually a correction of the earlier one.
      if (!live || p.start > live.start) live = p;
    } else if (p.start > nowSeconds) {
      if (!next || p.start < next.start) next = p;
    }
  }
  return { now: live, next };
}

// ---------------------------------------------------------------- the grid

/** Dense enough that a screen shows twenty-odd channels, tall enough that a
 *  programme title and its time both fit. */
export const GUIDE_ROW_HEIGHT = 34;
export const HALF_HOUR = 1800;
export const MIN_GUIDE_HOURS = 1;
export const MAX_GUIDE_HOURS = 6;

export interface TimeWindow {
  /** Unix seconds, inclusive. */
  from: number;
  /** Unix seconds, exclusive. */
  to: number;
}

export const clampHours = (hours: number): number =>
  Math.min(MAX_GUIDE_HOURS, Math.max(MIN_GUIDE_HOURS, Math.round(hours)));

/** The window containing `anchor`, snapped back to the half hour so the axis
 *  labels land on :00 and :30 instead of drifting with the clock. */
export function windowAt(anchor: number, hours: number): TimeWindow {
  const from = Math.floor(anchor / HALF_HOUR) * HALF_HOUR;
  return { from, to: from + clampHours(hours) * HOUR };
}

export function shiftWindow(win: TimeWindow, hours: number): TimeWindow {
  const by = hours * HOUR;
  return { from: win.from + by, to: win.to + by };
}

/** Pixels from the left of the track for an instant. Unclamped: callers that
 *  draw need to know when something is off the edge. */
export function xForTime(t: number, win: TimeWindow, width: number): number {
  const span = win.to - win.from;
  if (!(span > 0) || !(width > 0)) return 0;
  return ((t - win.from) / span) * width;
}

export function timeForX(x: number, win: TimeWindow, width: number): number {
  const span = win.to - win.from;
  if (!(span > 0) || !(width > 0)) return win.from;
  return win.from + (x / width) * span;
}

/** The half-hour marks inside the window, first one at or after `from`. */
export function ticks(win: TimeWindow, step = HALF_HOUR): number[] {
  if (!(step > 0) || !(win.to > win.from)) return [];
  const out: number[] = [];
  for (let t = Math.ceil(win.from / step) * step; t < win.to; t += step) out.push(t);
  return out;
}

export interface Span {
  left: number;
  width: number;
  /** The programme began before the window and is cut off on the left. */
  clippedStart: boolean;
  clippedEnd: boolean;
}

/** Where a programme sits in the track, clipped to the window.
 *
 *  `null` means "not in this window at all" — the caller draws nothing rather
 *  than a zero-width sliver at an edge. A zero-length programme (some
 *  guides contain these) is inside the window and gets a zero-width span; the grid
 *  skips drawing a block nobody could hit, but the arithmetic stays sane. */
export function programmeSpan(
  p: { start: number; stop: number },
  win: TimeWindow,
  width: number,
): Span | null {
  const end = Math.max(p.start, p.stop);
  if (end <= win.from || p.start >= win.to) return null;
  const left = Math.max(0, xForTime(p.start, win, width));
  const right = Math.min(width, xForTime(end, win, width));
  return {
    left,
    width: Math.max(0, right - left),
    clippedStart: p.start < win.from,
    clippedEnd: end > win.to,
  };
}

/** Pixels for the now-line, or `null` when now is outside the window — which
 *  it is the moment anyone pages forward, and the line must then be absent
 *  rather than pinned to an edge and lying. */
export function nowLineX(now: number, win: TimeWindow, width: number): number | null {
  if (now < win.from || now >= win.to) return null;
  return xForTime(now, win, width);
}

export type ProgrammeStatus = "past" | "live" | "future";

export function programmeStatus(
  p: { start: number; stop: number },
  now: number,
): ProgrammeStatus {
  if (now < p.start) return "future";
  if (now < Math.max(p.start, p.stop)) return "live";
  return "past";
}

/** What a click on a programme can do. Anything else is inert, and looks it —
 *  a finished programme on a channel with no archive is not a broken button. */
export type Playability = "play" | "catchup" | "inert";

export function playability(status: ProgrammeStatus, hasArchive: boolean): Playability {
  if (status === "live") return "play";
  if (status === "past" && hasArchive) return "catchup";
  return "inert";
}

/** `play_catchup` wants whole minutes. Never zero: a zero-length request would
 *  open a stream with nothing in it. */
export function catchupMinutes(p: { start: number; stop: number }): number {
  return Math.max(1, Math.round((Math.max(p.start, p.stop) - p.start) / 60));
}

/** Whether the archive still reaches back that far. `archiveDays` is null when
 *  the provider did not say, and a missing number is not permission to refuse. */
export function withinArchive(
  p: { start: number },
  now: number,
  archiveDays: number | null,
): boolean {
  if (archiveDays == null) return true;
  return p.start >= now - archiveDays * 86400;
}
