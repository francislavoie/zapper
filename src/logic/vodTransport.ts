// The numbers behind a movie's transport: its times, where a click on the
// seek bar lands, and how much of the bar is played and read ahead. Pure;
// checked in src/__checks__/vodTransport.test.mjs.
//
// Position and duration arrive rounded to whole seconds, so the readout moves
// once a second and never shows a fraction.

import type { OnDemandDto } from "../types";
import { clock } from "./format.ts";

/** What the skip buttons, ←/→ and the picture's side zones move a movie by,
 *  and what the icons say. A film is long, so the step is longer than a
 *  channel's. */
export const VOD_SKIP_SECONDS = 10;

/** The three times of the readout. Total and remaining are empty while the
 *  length is unknown: a readout of "58:12 / 0:00" would be a lie. */
export function vodTimes(
  position: number,
  duration: number | null,
): { elapsed: string; total: string; remaining: string } {
  const elapsed = clock(position);
  if (duration === null || !(duration > 0)) return { elapsed, total: "", remaining: "" };
  return {
    elapsed,
    total: clock(duration),
    remaining: `−${clock(Math.max(0, duration - position))}`,
  };
}

/** "58:12 / 2:01:04 · −1:02:52"; compact leaves out what is left. */
export function vodLabel(o: OnDemandDto, compact = false): string {
  const t = vodTimes(o.position, o.duration);
  if (!t.total) return t.elapsed;
  return compact ? `${t.elapsed} / ${t.total}` : `${t.elapsed} / ${t.total} · ${t.remaining}`;
}

/** The time a click at `fraction` of the bar's width asks for, kept inside
 *  the movie; null when there is no length to take a fraction of. */
export function seekTarget(fraction: number, duration: number | null): number | null {
  if (duration === null || !(duration > 0) || !Number.isFinite(fraction)) return null;
  return Math.min(1, Math.max(0, fraction)) * duration;
}

const fractionOf = (seconds: number | null, duration: number | null): number =>
  seconds === null || duration === null || !(duration > 0)
    ? 0
    : Math.min(1, Math.max(0, seconds / duration));

export const playedFraction = (o: OnDemandDto): number => fractionOf(o.position, o.duration);

export const bufferedFraction = (o: OnDemandDto): number => fractionOf(o.bufferedTo, o.duration);
