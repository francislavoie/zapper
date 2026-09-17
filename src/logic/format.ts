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

/** "-1:23" behind live, "LIVE" at the edge. */
export function liveOffset(behindLive: number): string {
  return behindLive < 1 ? "LIVE" : `-${clock(behindLive)}`;
}

/** The gap, said the way a person would: seconds under a minute, clock time
 *  past it. */
export function behindGap(behindLive: number): string {
  const s = Math.max(0, Math.round(behindLive));
  return s < 60 ? `${s}s` : clock(s);
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
