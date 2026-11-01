// Video is not a DOM element: mpv draws it beneath the webview, and the only
// way to move it is to hand Rust normalised 0-1 window coordinates. So the
// "where is the video" question is arithmetic on a DOM box, and arithmetic is
// checkable — see src/__checks__/rect.test.mjs.

import type { VideoRect } from "../types";

export const FULL: VideoRect = { x: 0, y: 0, w: 1, h: 1 };

export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

const clamp01 = (n: number) => Math.min(1, Math.max(0, n));

/** A DOM box in CSS pixels becomes a normalised rect. Clamped, because a box
 *  can legitimately sit partly offscreen mid-layout and mpv should not be
 *  asked to draw outside the window. */
export function normalisedRect(box: Box, winW: number, winH: number): VideoRect {
  if (!(winW > 0) || !(winH > 0)) return FULL;
  const x = clamp01(box.left / winW);
  const y = clamp01(box.top / winH);
  const w = clamp01((box.left + box.width) / winW) - x;
  const h = clamp01((box.top + box.height) / winH) - y;
  return { x, y, w: Math.max(0, w), h: Math.max(0, h) };
}

export function lerpRect(a: VideoRect, b: VideoRect, t: number): VideoRect {
  const k = Math.min(1, Math.max(0, t));
  // Snap the endpoints: floating point drift at t=1 would leave the video a
  // hair off its target and keep the "has it changed" check firing.
  if (k === 0) return a;
  if (k === 1) return b;
  return {
    x: a.x + (b.x - a.x) * k,
    y: a.y + (b.y - a.y) * k,
    w: a.w + (b.w - a.w) * k,
    h: a.h + (b.h - a.h) * k,
  };
}

/** The picture's move between the docked slot and the full stage, both
 *  ways: one curve, so leaving looks like entering run backwards.
 *
 *  In-out rather than ease-out. Ease-out spends the last third of its time
 *  covering the last few percent of the distance — a crawl of a pixel or
 *  two a frame — and every one of those frames is a resize of the video
 *  surface, at its largest on the way in. That crawl is where an uneven
 *  frame shows, as a hitch just before the picture settles. In-out keeps
 *  moving until much later and then lands. */
export function easeInOutCubic(t: number): number {
  const k = Math.min(1, Math.max(0, t));
  return k < 0.5 ? 4 * k * k * k : 1 - Math.pow(-2 * k + 2, 3) / 2;
}

/** The rect to draw `elapsed` ms into a dock/undock transition, and whether
 *  the transition is over. A zero or negative duration snaps. */
export function rectAt(
  from: VideoRect,
  to: VideoRect,
  elapsed: number,
  duration: number,
): { rect: VideoRect; done: boolean } {
  if (!(duration > 0) || elapsed >= duration) return { rect: to, done: true };
  if (elapsed <= 0) return { rect: from, done: false };
  return { rect: lerpRect(from, to, easeInOutCubic(elapsed / duration)), done: false };
}

/** Sub-pixel churn is not worth an IPC round trip every frame. */
export function rectEquals(a: VideoRect, b: VideoRect, eps = 0.0005): boolean {
  return (
    Math.abs(a.x - b.x) < eps &&
    Math.abs(a.y - b.y) < eps &&
    Math.abs(a.w - b.w) < eps &&
    Math.abs(a.h - b.h) < eps
  );
}

/** Whether a frame of a transition is worth an IPC call.
 *
 *  Mid-flight, sub-pixel churn is skipped. The final frame is never skipped:
 *  it is the one that has to land exactly on the target, or the video settles
 *  a hair away from its hole and stays there. */
export function shouldSend(
  current: VideoRect,
  next: VideoRect,
  done: boolean,
  eps = 0.0002,
): boolean {
  return done ? !rectEquals(current, next, 1e-9) : !rectEquals(current, next, eps);
}

/** Fit a 16:9 (or any) aspect inside a box, centred — mpv letterboxes on its
 *  own, but the dock placeholder should match what will actually be drawn so
 *  the transparent hole is the right shape. */
export function fitAspect(box: Box, aspect: number): Box {
  if (!(box.width > 0) || !(box.height > 0) || !(aspect > 0)) return box;
  const boxAspect = box.width / box.height;
  if (boxAspect > aspect) {
    const width = box.height * aspect;
    return { left: box.left + (box.width - width) / 2, top: box.top, width, height: box.height };
  }
  const height = box.width / aspect;
  return { left: box.left, top: box.top + (box.height - height) / 2, width: box.width, height };
}

/** Sends rects to the backend one at a time, always the newest.
 *
 *  A transition sends a rect every frame, and each is applied on the main
 *  thread as a resize of the video surface. While a resize is slow — the
 *  surface is large, or the main thread is busy — sends would queue behind
 *  it and then be applied in a burst, each one already stale: the picture
 *  stalls, then jumps through frames nobody needed, and the last one, the
 *  one that lands it, waits behind them all. Nor are concurrent commands
 *  guaranteed to arrive in the order they were sent. So: at most one in
 *  flight, and whatever arrives meanwhile replaces whatever was waiting. */
export function latestOnly<T>(send: (value: T) => Promise<unknown>): (value: T) => void {
  let busy = false;
  let waiting: { value: T } | null = null;
  const post = (value: T) => {
    if (busy) {
      waiting = { value };
      return;
    }
    busy = true;
    void send(value)
      .catch(() => {})
      .finally(() => {
        busy = false;
        const next = waiting;
        waiting = null;
        if (next) post(next.value);
      });
  };
  return post;
}
