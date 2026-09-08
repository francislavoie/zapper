// The two-zone timeline.
//
// A 3-day archive and a 4-minute buffer cannot share a linear scale: the
// buffer would be 0.09% of the width, about one pixel. So the bar is split at
// a seam — archive to the left, buffer to the right — and each zone maps its
// own span of time across its own span of pixels. Left of the seam a seek
// costs a reconnect (~2s); right of it, it is instant.
//
// Channels without catchup show the buffer zone alone, labelled.
//
// Checked in src/__checks__/scrubber.test.mjs.

export const ARCHIVE_FRACTION = 0.62;
export const DAY = 86400;

export interface TimelineInput {
  width: number;
  hasArchive: boolean;
  archiveDays: number | null;
  /** Seconds of scrollback mpv is holding. */
  bufferedBack: number;
  /** Unix seconds at the live edge. */
  now: number;
}

export interface Geometry {
  width: number;
  /** Pixel of the seam. Zero when there is no archive zone. */
  seamPx: number;
  /** Unix seconds at the far left. */
  archiveStart: number;
  /** Unix seconds at the seam: the oldest instant held locally. */
  bufferStart: number;
  /** Unix seconds at the right edge. */
  live: number;
  hasArchive: boolean;
}

export function geometry(t: TimelineInput): Geometry {
  const width = Math.max(0, t.width);
  const buffered = Math.max(0, t.bufferedBack);
  const bufferStart = t.now - buffered;
  const days = t.hasArchive ? Math.max(0, t.archiveDays ?? 0) : 0;
  const hasArchive = t.hasArchive && days > 0;
  return {
    width,
    seamPx: hasArchive ? width * ARCHIVE_FRACTION : 0,
    archiveStart: hasArchive ? t.now - days * DAY : bufferStart,
    bufferStart,
    live: t.now,
    hasArchive,
  };
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** Unix seconds under a pixel. */
export function timeAtPixel(g: Geometry, px: number): number {
  if (g.width <= 0) return g.live;
  const x = clamp(px, 0, g.width);

  if (g.hasArchive && x < g.seamPx) {
    if (g.seamPx <= 0) return g.bufferStart;
    const f = x / g.seamPx;
    return g.archiveStart + f * (g.bufferStart - g.archiveStart);
  }

  const bufferWidth = g.width - g.seamPx;
  if (bufferWidth <= 0) return g.live;
  const f = (x - g.seamPx) / bufferWidth;
  return g.bufferStart + f * (g.live - g.bufferStart);
}

/** The pixel for an instant. The inverse of timeAtPixel inside the bar. */
export function pixelAtTime(g: Geometry, t: number): number {
  if (g.width <= 0) return 0;
  const time = clamp(t, g.archiveStart, g.live);

  if (g.hasArchive && time < g.bufferStart) {
    const span = g.bufferStart - g.archiveStart;
    if (span <= 0) return 0;
    return ((time - g.archiveStart) / span) * g.seamPx;
  }

  const span = g.live - g.bufferStart;
  const bufferWidth = g.width - g.seamPx;
  // A buffer of zero length is a point in time; put the head at the live end.
  if (span <= 0) return g.width;
  return g.seamPx + ((time - g.bufferStart) / span) * bufferWidth;
}

export type Zone = "archive" | "buffer";

export function zoneAtPixel(g: Geometry, px: number): Zone {
  return g.hasArchive && px < g.seamPx ? "archive" : "buffer";
}

/** What a seek to this instant costs. The distinction is the whole reason the
 *  seam is drawn: left of it the stream is reopened from the server. */
export function seekCost(g: Geometry, t: number): "instant" | "reconnect" {
  return g.hasArchive && t < g.bufferStart ? "reconnect" : "instant";
}

/** Seconds behind the live edge for an instant, which is what `seek_relative`
 *  and the readout both speak in. */
export function behindLiveAt(g: Geometry, t: number): number {
  return Math.max(0, g.live - t);
}

/** Catchup is requested as a start time plus a duration in minutes; the
 *  backend builds the timeshift URL from those. Rounded down to the minute
 *  because the server's `start` parameter has minute resolution. */
export function catchupRequest(
  g: Geometry,
  t: number,
): { start: number; durationMinutes: number } {
  const start = Math.floor(clamp(t, g.archiveStart, g.bufferStart) / 60) * 60;
  const minutes = Math.max(1, Math.ceil((g.live - start) / 60));
  return { start, durationMinutes: minutes };
}
