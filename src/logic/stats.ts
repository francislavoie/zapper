// The arithmetic behind the diagnostics panel.
//
// A single reading says the buffer is 12 seconds deep; a minute of readings
// says whether it is draining, and detecting a stutter is exactly that
// difference. So the panel draws a history — and does not gather one. That
// happens in the backend, on the player's own heartbeat, so the graphs are
// full the moment the panel opens instead of starting from nothing at the
// one moment somebody wanted to look. See `player::health` on the Rust side.
//
// What is left here is presentation: turning a list of samples into a path,
// and numbers into words. All pure, and checked in
// src/__checks__/stats.test.mjs.

import type { StatsSampleDto } from "../types";

/** How often the panel asks.
 *
 *  Matched to the rate the backend records at (a 250ms heartbeat, every
 *  second one) so that a poll collects about one new sample: faster only
 *  re-fetches a history that has not changed, and slower lets the graph lag
 *  behind the record it is drawing. */
export const STATS_POLL_MS = 500;

/** How many readings the graphs hold. At the sample rate, one minute — long
 *  enough to see a drain and short enough to still be about now.
 *
 *  Mirrored by `HISTORY_LENGTH` in `crates/zapper/src/player/health.rs`, which is
 *  what actually bounds the ring. This one sizes the horizontal axis: a
 *  sample is one slot wide, so the two have to agree or a full history draws
 *  a line that stops short of the edge of its box. */
export const HISTORY_LENGTH = 120;

/** One point on a graph, as the backend hands it over. */
export type Sample = StatsSampleDto;

/** An SVG polyline through `values`, scaled to a `width` x `height` box.
 *
 *  The newest sample is at the right, and a short history is drawn at the
 *  right-hand end rather than stretched across the box — otherwise a graph
 *  that has just started rescales horizontally on every tick and nothing
 *  holds still long enough to read.
 *
 *  `ceiling` fixes the vertical scale where there is a meaningful maximum
 *  (buffer health against the cache limit). Without one the scale is the
 *  largest value seen, which is right for a rate nobody knows in advance. */
export function sparkline(
  values: readonly number[],
  width: number,
  height: number,
  ceiling?: number,
): string {
  if (values.length === 0 || width <= 0 || height <= 0) return "";
  const slots = Math.max(HISTORY_LENGTH - 1, 1);
  const top = Math.max(ceiling ?? Math.max(...values), Number.EPSILON);
  const step = width / slots;
  // Right-aligned: the last value sits on the right edge.
  const firstX = width - (values.length - 1) * step;
  return values
    .map((v, i) => {
      const x = firstX + i * step;
      const clamped = Math.min(Math.max(v, 0), top);
      const y = height - (clamped / top) * height;
      return `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
}

/** Seconds between graph samples: two heartbeats of 250ms (TICKS_PER_SAMPLE
 *  in crates/zapper/src/player/health.rs). */
export const SAMPLE_SECONDS = 0.5;

/** Where sample `index` of `count` sits across a graph, 0-1. The inverse of
 *  [`sampleAt`]; the same right-aligned layout `sparkline` draws. */
export function sampleX(index: number, count: number): number {
  const slots = Math.max(HISTORY_LENGTH - 1, 1);
  return 1 - (count - 1 - index) / slots;
}

/** Which sample is under a pointer at `fraction` (0-1) across a graph, or
 *  null where the graph has no data yet. A part-filled graph is drawn from
 *  the right, so its left side is empty and points at nothing. */
export function sampleAt(fraction: number, count: number): number | null {
  if (count <= 0 || !Number.isFinite(fraction)) return null;
  const slots = Math.max(HISTORY_LENGTH - 1, 1);
  const first = sampleX(0, count);
  const half = 0.5 / slots;
  if (fraction < first - half) return null;
  const i = Math.round((fraction - first) * slots);
  return Math.min(count - 1, Math.max(0, i));
}

// ------------------------------------------------------------- formatting

/** A dash, so a missing reading reads as "this stream does not say" rather
 *  than as zero. */
export const NONE = "—";

export function bitrate(bps: number | null | undefined): string {
  if (bps === null || bps === undefined || !Number.isFinite(bps) || bps <= 0) return NONE;
  if (bps >= 1_000_000) return `${(bps / 1_000_000).toFixed(2)} Mbps`;
  return `${Math.round(bps / 1000)} kbps`;
}

export function bytes(n: number | null | undefined): string {
  if (n === null || n === undefined || !Number.isFinite(n) || n < 0) return NONE;
  if (n >= 1024 ** 3) return `${(n / 1024 ** 3).toFixed(2)} GB`;
  if (n >= 1024 ** 2) return `${(n / 1024 ** 2).toFixed(1)} MB`;
  if (n >= 1024) return `${Math.round(n / 1024)} KB`;
  return `${Math.round(n)} B`;
}

export function rate(bps: number | null | undefined): string {
  if (bps === null || bps === undefined || !Number.isFinite(bps) || bps < 0) return NONE;
  return `${bytes(bps)}/s`;
}

export function seconds(s: number | null | undefined, places = 2): string {
  if (s === null || s === undefined || !Number.isFinite(s)) return NONE;
  return `${s.toFixed(places)}s`;
}

export function count(n: number | null | undefined): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return NONE;
  return Math.round(n).toLocaleString();
}

export function text(v: string | null | undefined): string {
  return v === null || v === undefined || v === "" ? NONE : v;
}

/** "1920x1080@50" plus what the renderer is actually achieving, when the two
 *  disagree — which is the readout that says frames are being dropped. */
export function resolutionLine(
  width: number | null,
  height: number | null,
  containerFps: number | null | undefined,
  measuredFps: number | null | undefined,
): string {
  if (!width || !height) return NONE;
  const declared = containerFps && containerFps > 0 ? `@${containerFps.toFixed(2).replace(/\.?0+$/, "")}` : "";
  const measured =
    measuredFps && measuredFps > 0 ? ` (${measuredFps.toFixed(1)} fps measured)` : "";
  return `${width}×${height}${declared}${measured}`;
}
