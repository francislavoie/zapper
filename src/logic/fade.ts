// When the controls are on screen. Three rules, and the last two are the ones
// people notice: idle for five seconds hides them, but leaving the window or
// touching a screen edge in fullscreen hides them *immediately* — waiting five
// seconds after the pointer has gone somewhere else is just clutter.
//
// Checked in src/__checks__/fade.test.mjs.

export const IDLE_MS = 5000;

export interface FadeState {
  visible: boolean;
  /** Timestamp of the last pointer activity, in ms. */
  lastActivity: number;
  /** Held open: the pointer is over a control, or a drag is in progress. */
  pinned: boolean;
}

export type FadeEvent =
  | { type: "activity" }
  | { type: "leave" }
  | { type: "edge" }
  | { type: "tick" }
  | { type: "pin"; pinned: boolean };

export function initialFade(now: number): FadeState {
  return { visible: true, lastActivity: now, pinned: false };
}

export function nextFade(s: FadeState, e: FadeEvent, now: number): FadeState {
  switch (e.type) {
    case "activity":
      return { ...s, visible: true, lastActivity: now };
    case "leave":
      // No grace period: the pointer has left, so the controls are not about
      // to be used.
      return { ...s, visible: false, pinned: false };
    case "edge":
      // Fullscreen only. Pinning does not survive it either — the pointer is
      // on its way out of the picture.
      return { ...s, visible: false, pinned: false };
    case "pin":
      return e.pinned
        ? { ...s, pinned: true, visible: true, lastActivity: now }
        : { ...s, pinned: false, lastActivity: now };
    case "tick":
      if (s.pinned || !s.visible) return s;
      return now - s.lastActivity >= IDLE_MS ? { ...s, visible: false } : s;
  }
}

/** True when the pointer is within `margin` of any window edge. In fullscreen
 *  that means the user is reaching for something outside the picture, which is
 *  the cue to get the controls out of the way. */
export function atScreenEdge(
  x: number,
  y: number,
  width: number,
  height: number,
  margin = 4,
): boolean {
  return x <= margin || y <= margin || x >= width - margin || y >= height - margin;
}
