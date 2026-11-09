// Programme search results: what each one says and what choosing it does.
//
// Matching and ordering are the backend's (an FTS5 index over guide titles,
// see crates/zapper-core/src/catalog/guide_search.rs); this is only the wording and
// the arithmetic for taking the guide to an airing.
//
// Checked in src/__checks__/programmes.test.mjs.

import type { AiringDto, ProgrammeDto } from "../types";
import { hhmm } from "./format.ts";
import { GUIDE_FUTURE_HOURS, HALF_HOUR, HOUR, anchorFor, clampOffset, dayLabel } from "./guide.ts";

/** The two kinds of search result the list can show. */
export type SearchTab = "channels" | "programmes";

/** What choosing a hit does: tune to what is on, take the guide to what is
 *  coming, or play what has finished from the archive. */
export type HitAction = "tune" | "guide" | "catchup";

export function hitAction(airing: AiringDto): HitAction {
  if (airing === "live") return "tune";
  if (airing === "upcoming") return "guide";
  return "catchup";
}

/** Short enough for the palette's narrow first column. */
export function airingTag(airing: AiringDto): string {
  if (airing === "live") return "on now";
  if (airing === "upcoming") return "upcoming";
  return "catch-up";
}

/** Said on hover, so a click on an upcoming airing opening the guide is not
 *  a surprise. */
export function hitTip(airing: AiringDto): string {
  const action = hitAction(airing);
  if (action === "tune") return "On now: tune in";
  if (action === "guide") return "Show it in the guide";
  return "Play it from the archive";
}

/** When, as the list writes it: "until 21:30" for what is on, whose tag
 *  already says "on now"; else the day and the start — "Today 20:00",
 *  "Tomorrow 06:00", "Yesterday 21:00", or a weekday further out. The tag
 *  says which way a weekday points, and guides and archives rarely reach
 *  past a week either way. */
export function airingWhen(
  hit: { airing: AiringDto; programme: Pick<ProgrammeDto, "start" | "stop"> },
  now: number,
): string {
  const p = hit.programme;
  if (hit.airing === "live") {
    return p.stop > p.start ? `until ${hhmm(p.stop)}` : "now";
  }
  return `${dayLabel(p.start, now) || "Today"} ${hhmm(p.start)}`;
}

/** The guide offset (from `anchor`, as the store keeps it) that opens the
 *  guide on an airing: the window starts on the half hour the programme
 *  starts in, so it sits at the left edge with its whole length to the right.
 *  Clamped to the guide's reach like any other offset. */
export function guideOffsetFor(start: number, anchor: number, hours: number): number {
  return clampOffset(Math.floor(start / HALF_HOUR) * HALF_HOUR - anchor, hours);
}

/** Whether the guide reaches far enough ahead to show an airing at all.
 *  Guides list a week ahead but the grid scrolls two days; past that, the
 *  guide can only open at its far edge, and should say why. */
export function inGuideReach(start: number, now: number): boolean {
  return start < anchorFor(now) + GUIDE_FUTURE_HOURS * HOUR;
}

/** Which category the guide shows to land on a channel. The one on screen
 *  when the channel is in it — every channel is in "all" — so jumping to an
 *  airing does not throw away the list being browsed; else the channel's
 *  own. A channel in no category is only in "all". */
export function focusCategory(
  current: string,
  channelCategory: string | null,
  all: string,
): string {
  if (current === all || current === channelCategory) return current;
  return channelCategory ?? all;
}

/** The guide's row scroll that puts row `index` in the middle of a viewport
 *  `height` tall, rows `rowHeight` apart. Never negative; the bottom is the
 *  scroller's to clamp. */
export function centredScrollTop(index: number, rowHeight: number, height: number): number {
  return Math.max(0, Math.round(index * rowHeight - (height - rowHeight) / 2));
}
