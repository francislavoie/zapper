// Guide arithmetic: time to pixels, and which programme is on.
//
// Everything here is pure so the grid's geometry can be checked without a
// browser — see src/__checks__/guide.test.mjs.

import type { ChannelDto, ProgrammeDto } from "../types";
import type { WheelLike } from "./wheel";

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

/** What a list row shows for a channel: the guide's answer when it has one,
 *  else the channel's own current programme, but only while it is still on. A
 *  page fetched an hour ago can carry one that has since ended. */
export function rowNowNext(
  channel: Pick<ChannelDto, "nowPlaying">,
  programmes: readonly ProgrammeDto[] | undefined,
  now: number,
): NowNext {
  if (programmes) return splitNowNext(programmes, now);
  const own = channel.nowPlaying;
  return { now: own && own.start <= now && now < own.stop ? own : null, next: null };
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

// ------------------------------------------------------- scrolling in time
//
// The guide's horizontal position is a number of seconds, not a scrollLeft.
// There is no wide strip of programmes being slid under a viewport: the
// window is recomputed from the offset and everything in it — ticks, blocks,
// the now-line — is placed by the same xForTime() on the same render, so the
// ruler and the rows cannot drift apart, and only what is on screen is ever
// in the DOM however far the guide can reach.
//
// The offset is measured from `anchor`, the half hour the clock is in (the
// left edge windowAt() gives). Zero means "follow now", which is what the
// guide opens on. Anything else is a place somebody scrolled to, and must
// stay put when the clock rolls over the half hour — see rebaseOffset().

/** How far back the guide reaches. Catchup archives are usually a day to a
 *  week deep, but the XMLTV providers serve typically carries a day of
 *  history, and scrolling further would mostly be scrolling through "no
 *  guide data". A day covers "what was on last night", which is what
 *  someone scrolling back is after; anything older is a search, not a
 *  scroll. */
export const GUIDE_PAST_HOURS = 24;
/** How far ahead. Panel guides run two to seven days out, but past two days
 *  the listings are placeholders as often as not, and the far edge should be
 *  reachable with a few flicks rather than a minute of swiping. */
export const GUIDE_FUTURE_HOURS = 48;

/** The half hour the clock is in: where an offset of zero starts. */
export const anchorFor = (now: number): number => Math.floor(now / HALF_HOUR) * HALF_HOUR;

/** Keep an offset inside the reach above, for a window `hours` wide. The far
 *  edge clamps the window's right side, not its left, so the last screen is
 *  a full screen rather than one sliding off into nothing. Not rounded: a
 *  pixel is some nine-odd seconds, and rounding each of a touchpad's many
 *  one-pixel events would leave the programmes trailing the fingers. */
export function clampOffset(offset: number, hours: number): number {
  const lo = -GUIDE_PAST_HOURS * HOUR;
  const hi = Math.max(lo, GUIDE_FUTURE_HOURS * HOUR - clampHours(hours) * HOUR);
  return Math.min(hi, Math.max(lo, offset));
}

/** Carry an offset across a change of anchor. Without this, the moment the
 *  clock passed :00 or :30 a guide scrolled back to last night would jump
 *  half an hour under the reader's eyes. Following now (zero) keeps
 *  following; an unknown previous anchor has nothing to correct. */
export function rebaseOffset(
  offset: number,
  from: number | null,
  to: number,
  hours: number,
): number {
  if (offset === 0 || from === null || from === to) return offset;
  return clampOffset(offset - (to - from), hours);
}

/** The window on screen, for an offset from the clock's anchor. */
export function guideWindow(now: number, hours: number, offset: number): TimeWindow {
  return shiftWindow(windowAt(now, hours), clampOffset(offset, hours) / HOUR);
}

/** Whether the window shows now — which is what the "Now" button lights up
 *  for, rather than for an offset of exactly zero: scrolled ten minutes
 *  sideways, now is still on screen and the button should say so. */
export const showsNow = (now: number, win: TimeWindow): boolean =>
  now >= win.from && now < win.to;

/** The horizontal part of a wheel event, in pixels, or null if it is not the
 *  guide's to take.
 *
 *  Three gestures mean sideways: a touchpad swipe or a tilt wheel (deltaX),
 *  and Shift with an ordinary wheel. Some engines already turn Shift+wheel
 *  into deltaX and some leave it in deltaY, so Shift takes whichever is set.
 *  An unshifted gesture is sideways only if it is mostly sideways, which is
 *  exactly the case the page's vertical wheel handling (logic/wheel.ts)
 *  declines, so the two never both act on one event: a diagonal swipe moves
 *  one axis, never both. Ctrl and Meta are zoom, and left alone. */
export function horizontalPixels(
  e: Pick<WheelLike, "deltaX" | "deltaY" | "deltaMode" | "ctrlKey" | "metaKey" | "shiftKey">,
  pageWidth: number,
): number | null {
  if (e.ctrlKey || e.metaKey) return null;
  let d: number;
  if (e.shiftKey) d = e.deltaX !== 0 ? e.deltaX : e.deltaY;
  else if (e.deltaX !== 0 && Math.abs(e.deltaX) > Math.abs(e.deltaY)) d = e.deltaX;
  else return null;
  if (d === 0) return null;
  // Lines and pages with the sizes wheel.ts uses; WebKitGTK sends pixels.
  if (e.deltaMode === 1) return d * 40;
  if (e.deltaMode === 2) return d * pageWidth;
  return d;
}

/** Seconds for a horizontal movement of `px` on a track `width` wide showing
 *  `hours`: the axis's own scale, so a swipe moves the programmes exactly as
 *  far as the fingers went. */
export function secondsForPixels(px: number, hours: number, width: number): number {
  if (!(width > 0)) return 0;
  return (px * clampHours(hours) * HOUR) / width;
}

// ------------------------------------------------------- fetching in chunks
//
// Programme data is asked for in fixed, clock-aligned chunks rather than for
// the window on screen. A window that moves by the pixel would otherwise be
// a new question every frame; a chunk only changes when the scroll crosses
// into the next one, and what was fetched for it stays good as long as the
// guide is near it.

/** Six hours: a whole screen at the widest setting fits in two chunks, and a
 *  day of scrolling is four requests per screen of rows. */
export const GUIDE_CHUNK = 6 * HOUR;

/** The chunks (by start) covering the window plus `margin` seconds either
 *  side — the margin is prefetch, so a swipe arrives at programmes already
 *  loaded rather than at a shimmer. */
export function chunksFor(win: TimeWindow, margin = 0, chunk = GUIDE_CHUNK): number[] {
  if (!(chunk > 0) || !(win.to > win.from)) return [];
  const first = Math.floor((win.from - margin) / chunk) * chunk;
  const out: number[] = [];
  for (let c = first; c < win.to + margin; c += chunk) out.push(c);
  return out;
}

/** One row's programmes from several chunks, as the window will draw them:
 *  only those overlapping the window, each once, in order. A programme that
 *  straddles a chunk boundary comes back from both requests, and the guide
 *  keys blocks by start time, so duplicates are dropped by start. */
export function visibleProgrammes(
  lists: readonly (readonly ProgrammeDto[])[],
  win: TimeWindow,
): ProgrammeDto[] {
  const byStart = new Map<number, ProgrammeDto>();
  for (const list of lists) {
    for (const p of list) {
      if (Math.max(p.start, p.stop) <= win.from || p.start >= win.to) continue;
      if (!byStart.has(p.start)) byStart.set(p.start, p);
    }
  }
  return [...byStart.values()].sort((a, b) => a.start - b.start);
}

/** "Yesterday", "Tomorrow", a weekday, or nothing for today — so a header
 *  reading 21:00–00:00 says which evening once the guide can reach two. By
 *  calendar day in local time, rounded so a daylight-saving day of 23 or 25
 *  hours still counts as one. */
export function dayLabel(t: number, now: number): string {
  const day = (s: number) => {
    const d = new Date(s * 1000);
    return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  };
  const diff = Math.round((day(t) - day(now)) / 86_400_000);
  if (diff === 0) return "";
  if (diff === -1) return "Yesterday";
  if (diff === 1) return "Tomorrow";
  return new Date(t * 1000).toLocaleDateString(undefined, { weekday: "short" });
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

// ------------------------------------------------------------ easing in time
//
// The store holds where the guide is going; the grid draws where it has got
// to. A wheel notch in WebKitGTK is some fifty pixels — a quarter of an hour
// at the default width — and applied at once that reads as the guide jumping
// rather than moving. So the grid eases its drawn window toward the target
// instead, in absolute time: it chases the target window's left edge, not
// the offset, so the half-hour rebase (which keeps that edge where it is)
// never animates, while "now" moving on does.
//
// Each notch moves the target, and the drawn window keeps chasing whatever
// the target now is. A fast spin therefore covers the whole distance — it is
// never restarted from wherever the animation had got to, which is what made
// WebKit's own smooth scrolling crawl behind the wheel.

/** How quickly the drawn window closes on the target: the gap shrinks by a
 *  factor of e every this many milliseconds, so it is ~95% of the way there
 *  in three of them — over in about a fifth of a second. */
export const GUIDE_EASE_MS = 70;

/** One frame's step of an exponential approach. `snap` is how close counts
 *  as there: inside it the target is returned exactly, so the animation ends
 *  on the number it was aiming at rather than a hair short of it forever. A
 *  negative frame time (a requestAnimationFrame stamp from before the
 *  animation was started) is no time at all, never a step backwards. */
export function easeToward(
  current: number,
  target: number,
  dtMs: number,
  tauMs = GUIDE_EASE_MS,
  snap = 1,
): number {
  const gap = target - current;
  if (!(Math.abs(gap) > snap) || !(tauMs > 0)) return target;
  const next = target - gap * Math.exp(-Math.max(0, dtMs) / tauMs);
  return Math.abs(target - next) <= snap ? target : next;
}

// ---------------------------------------------------------- the scrollbar
//
// A bar along the bottom of the guide standing for its whole reach, from
// GUIDE_PAST_HOURS back to GUIDE_FUTURE_HOURS ahead, with a thumb for the
// window on screen. The thumb is as wide as the window is a fraction of the
// reach, but never narrower than something a pointer can hold. Because of
// that floor, the thumb's left edge travels across the track minus its own
// width, and every mapping below goes through the same scale so that the
// thumb, the now mark and a drag can never disagree.

/** The whole reach, in seconds. */
export const GUIDE_REACH = (GUIDE_PAST_HOURS + GUIDE_FUTURE_HOURS) * HOUR;

/** The narrowest thumb, in pixels: a 1h window over three days would
 *  otherwise be a sliver. */
export const GUIDE_THUMB_MIN_PX = 24;

export interface Thumb {
  left: number;
  width: number;
}

const lowestOffset = (): number => -GUIDE_PAST_HOURS * HOUR;

function thumbWidth(hours: number, track: number, minPx: number): number {
  if (!(track > 0)) return 0;
  return Math.min(track, Math.max(minPx, (track * clampHours(hours) * HOUR) / GUIDE_REACH));
}

/** Pixels the thumb moves per second of offset. Zero when the window is the
 *  whole reach and there is nowhere to go. */
function thumbScale(hours: number, track: number, minPx: number): number {
  const span = clampOffset(Infinity, hours) - lowestOffset();
  const travel = track - thumbWidth(hours, track, minPx);
  return span > 0 && travel > 0 ? travel / span : 0;
}

/** The thumb for a window at `offset` seconds from the anchor. */
export function scrollThumb(
  offset: number,
  hours: number,
  track: number,
  minPx = GUIDE_THUMB_MIN_PX,
): Thumb {
  const width = thumbWidth(hours, track, minPx);
  const scale = thumbScale(hours, track, minPx);
  return { left: (clampOffset(offset, hours) - lowestOffset()) * scale, width };
}

/** Where a drag that started with the window at `startOffset` puts it, after
 *  the pointer has moved `dx` pixels: one to one with the thumb, clamped to
 *  the reach. */
export function dragOffset(
  startOffset: number,
  dx: number,
  hours: number,
  track: number,
  minPx = GUIDE_THUMB_MIN_PX,
): number {
  const scale = thumbScale(hours, track, minPx);
  if (!(scale > 0)) return clampOffset(startOffset, hours);
  return clampOffset(startOffset + dx / scale, hours);
}

/** Where an instant sits along the bar, for the now mark: the point that
 *  would be the thumb's centre with the window centred on it. Unclamped, so a
 *  caller can tell when it is off the end. */
export function scrollbarX(
  t: number,
  anchor: number,
  hours: number,
  track: number,
  minPx = GUIDE_THUMB_MIN_PX,
): number {
  const half = (clampHours(hours) * HOUR) / 2;
  const scale = thumbScale(hours, track, minPx);
  return (t - anchor - half - lowestOffset()) * scale + thumbWidth(hours, track, minPx) / 2;
}

/** Which way a click on the bar pages: back if left of the thumb, forward if
 *  right of it, nowhere if on it. */
export function pageDirection(x: number, thumb: Thumb): -1 | 0 | 1 {
  if (x < thumb.left) return -1;
  if (x > thumb.left + thumb.width) return 1;
  return 0;
}
