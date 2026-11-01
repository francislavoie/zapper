// Display formatting. Pure; exercised by src/__checks__/format.test.mjs.

import type { PlayerStateDto, QualityDto } from "../types";

/** The badge shown on a channel row. `null` quality means never opened, which
 *  is unknown rather than bad, so it renders nothing at all. Channel names lie
 *  about resolution (`CA: HARBOUR SPORTS 1 4K` may well be 720p); only
 *  measured height is allowed to speak here. */
export function qualityBadge(q: QualityDto | null): string | null {
  if (!q) return null;
  const h = q.height;
  let label: string | null = null;
  if (h != null) {
    if (h >= 2000) label = "4K";
    else if (h >= 1400) label = "1440p";
    else if (h >= 1000) label = "1080p";
    else if (h >= 700) label = "720p";
    else if (h >= 500) label = "576p";
    else label = `${h}p`;
  }
  if (q.isHdr) label = label ? `${label} HDR` : "HDR";
  return label;
}

/** True when a channel has been tried and has failed more often than not.
 *  Never true for an unopened channel: no attempts is not a bad record. */
export function looksBroken(q: QualityDto | null): boolean {
  if (!q) return false;
  const tries = q.okCount + q.failCount;
  return tries >= 3 && q.failCount > q.okCount;
}

/** The measured-truth line the player reveals on hover. Empty string when the
 *  player has measured nothing yet. */
export function measuredLine(p: PlayerStateDto): string {
  const bits: string[] = [];
  if (p.width != null && p.height != null) bits.push(`${p.width}×${p.height}`);
  if (p.fps != null) bits.push(`${trimFps(p.fps)} fps`);
  if (p.codec) bits.push(p.codec);
  return bits.join(" · ");
}

function trimFps(fps: number): string {
  const r = Math.round(fps * 100) / 100;
  return Number.isInteger(r) ? String(r) : r.toFixed(2).replace(/0+$/, "");
}

/** mm:ss, or h:mm:ss past an hour. Negative clamps to zero — behindLive can
 *  arrive fractionally negative from the demuxer. */
export function clock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const mm = h > 0 ? String(m).padStart(2, "0") : String(m);
  return h > 0
    ? `${h}:${mm}:${String(sec).padStart(2, "0")}`
    : `${mm}:${String(sec).padStart(2, "0")}`;
}

/** A span of time in words: "42 s", "4 min", "10 min 40 s", "1 h 05 min".
 *
 *  For durations someone reads rather than a position someone scrubs to —
 *  "640 s" makes the reader do the division, and "10:40" reads as a time of
 *  day as often as a length. Rounded to the second under an hour and to the
 *  minute past it, where seconds stop meaning anything. Negative and
 *  non-finite clamp to zero. */
export function duration(seconds: number): string {
  // Compact — "11m12s", not "11 min 12 s" — because these sit in narrow
  // readouts and rows, and the unit letters are what keep it from reading
  // as a clock, which "11:12" did.
  const s = Number.isFinite(seconds) ? Math.max(0, Math.round(seconds)) : 0;
  if (s < 60) return `${s}s`;
  if (s < 3600) {
    const m = Math.floor(s / 60);
    const rest = s % 60;
    return rest === 0 ? `${m}m` : `${m}m${String(rest).padStart(2, "0")}s`;
  }
  const totalMin = Math.round(s / 60);
  const h = Math.floor(totalMin / 60);
  const m = totalMin % 60;
  return m === 0 ? `${h}h` : `${h}h${String(m).padStart(2, "0")}m`;
}

/** A span that is usually small but can run to minutes: `places` decimals
 *  under a minute, where the fraction still means something, and words
 *  ("10 min 40 s") from there up. */
export function span(s: number | null | undefined, places = 1): string {
  if (s === null || s === undefined || !Number.isFinite(s)) return "—";
  return s < 59.5 ? `${s.toFixed(places)}s` : duration(s);
}

/** "Live" at the edge, "-1m05s" behind it.
 *
 *  Takes the backend's `atLiveEdge`, not a threshold of its own. Deciding
 *  from `behindLive` here flickered between "-0:02" and "LIVE" as the
 *  seekable end sawtoothed, which the controls bar had already stopped doing
 *  by using the backend's hysteresis — and since live now sits a few seconds
 *  back on purpose, a bare threshold would never say "Live" at all. */
export function liveOffset(atLiveEdge: boolean, behindLive: number): string {
  return atLiveEdge ? "Live" : `-${duration(behindLive)}`;
}

/** The gap, said the way a person would: "4s", "3m05s". */
export function behindGap(behindLive: number): string {
  return duration(behindLive);
}

export interface LiveState {
  /** Red dot, or grey. */
  live: boolean;
  /** What the control says out loud: its title and its accessible name.
   *  Colour alone is not a message — a red dot and a grey one are the same dot
   *  to plenty of people. */
  label: string;
  /** Whether there is anywhere to jump to. At the edge there is not, and the
   *  control goes inert rather than pretending otherwise. */
  canJump: boolean;
}

/** What the live indicator shows.
 *
 *  `atLiveEdge` comes from the backend and is the only input to liveness.
 *  `behindLive` is measured against the end of mpv's seekable window, which
 *  advances in chunks as the demuxer reads, so it sawtooths by a couple of
 *  seconds during perfectly ordinary playback: any threshold taken here would
 *  sit inside that sawtooth and flip the indicator on every tick. The backend
 *  applies hysteresis — 3s to become live, 8s to stop being live — and the gap
 *  between those two numbers is the whole reason the dot holds still.
 *
 *  So `behindLive` is displayed, never thresholded. */
export function liveState(atLiveEdge: boolean, behindLive: number): LiveState {
  if (atLiveEdge) return { live: true, label: "Live", canJump: false };
  return { live: false, label: `Behind live by ${behindGap(behindLive)}`, canJump: true };
}

/** Wall-clock HH:MM for a unix-seconds instant, in the local zone. */
export function hhmm(unixSeconds: number): string {
  const d = new Date(unixSeconds * 1000);
  return `${String(d.getHours()).padStart(2, "0")}:${String(
    d.getMinutes(),
  ).padStart(2, "0")}`;
}

/** How far through a programme we are, 0-1, for the row's progress hairline. */
export function programmeProgress(
  start: number,
  stop: number,
  nowSeconds: number,
): number {
  if (!(stop > start)) return 0;
  const t = (nowSeconds - start) / (stop - start);
  return Math.min(1, Math.max(0, t));
}

/** Volume can exceed 100 (mpv's volume-max), so the readout must not clamp at
 *  100 — it clamps at the ceiling the caller passes. */
export function clampVolume(v: number, max: number): number {
  return Math.min(max, Math.max(0, Math.round(v)));
}

/** The badge for whatever is playing, from what the player actually measured.
 *  Same rule as the row badge — height decides, a name proves nothing — and
 *  nothing measured means no badge at all. */
export function playerBadge(p: PlayerStateDto): string | null {
  if (p.width == null && p.height == null) return null;
  return qualityBadge({
    codec: p.codec,
    width: p.width,
    height: p.height,
    fps: p.fps,
    isHdr: false,
    okCount: 0,
    failCount: 0,
  });
}

/** The two captions under the scrubber: what the left of the bar is, and
 *  what the right of it is.
 *
 *  `compact` is the docked picture's bar, a fraction of the player's width:
 *  the same two facts in the fewest words that still say them — how far back
 *  the archive goes, and how much is held locally. */
export function scrubLabels(o: {
  hasArchive: boolean;
  archiveDays: number | null;
  bufferedBack: number;
  hasChannel: boolean;
  compact: boolean;
}): [string, string] {
  const held = duration(o.bufferedBack);
  if (o.hasArchive) {
    return o.compact
      ? [`${o.archiveDays}d archive`, `${held} buffer`]
      : [`${o.archiveDays}-day archive · seek reconnects`, `buffer ${held} · instant`];
  }
  if (o.compact) return [o.hasChannel ? "no catchup" : "local buffer", `${held} back`];
  return [
    `local buffer only${o.hasChannel ? " — no catchup on this channel" : ""}`,
    `${held} of scrollback`,
  ];
}
