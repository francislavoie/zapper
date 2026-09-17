// Application zoom: one number that scales the whole interface.
//
// Everything in styles.css is sized in rem, and the root font size is
// `--zoom` times ZOOM_BASE_PX, so a single custom property moves type, padding,
// rows, the rail, the toolbar and the docked video slot together. Nothing is
// special-cased per component, and nothing is scaled by transform — a
// transform would blur text and, worse, leave the layout boxes where they
// were, while the video rect handed to mpv is measured from those boxes.
//
// Zoom is deliberately NOT in SettingsDto. That contract is Rust-side and
// synced; how big this screen wants its type is a property of this machine, so
// it lives in local storage.
//
// Checked in src/__checks__/zoom.test.mjs.

export const ZOOM_DEFAULT = 1;
export const ZOOM_MIN = 0.7;
export const ZOOM_MAX = 2;

/** The root font size at 100%. Every rem in the stylesheet is against this. */
export const ZOOM_BASE_PX = 16;

/** Where a step lands. Stops rather than a fixed multiplier, because a
 *  multiplier either crawls at the bottom or jumps at the top, and because the
 *  useful sizes are known: this is a TV app read from a couch as often as from
 *  a desk. */
export const ZOOM_STOPS: readonly number[] = [
  0.7, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2,
];

/** Floating-point slack: stored values round-trip through a string, so
 *  "is this already a stop" cannot be an equality test. */
const EPS = 1e-6;

export const ZOOM_STORAGE_KEY = "zapper.zoom";

export type ZoomCommand = "in" | "out" | "reset";

export function clampZoom(zoom: number): number {
  // NaN has no direction to clamp towards, so it falls back to 100%; an
  // infinity does, and clamps like any other out-of-range number.
  if (Number.isNaN(zoom)) return ZOOM_DEFAULT;
  return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, zoom));
}

/** The next stop in a direction. Clamped at both ends rather than wrapping:
 *  holding Ctrl+- should stop at the smallest size, not spring back to the
 *  largest. A value that is not itself a stop — an old stored one, say — moves
 *  to the nearest stop past it, so the steps stay predictable either way. */
export function nextZoom(current: number, command: ZoomCommand): number {
  if (command === "reset") return ZOOM_DEFAULT;
  const now = clampZoom(current);
  if (command === "in") {
    for (const stop of ZOOM_STOPS) if (stop > now + EPS) return stop;
    return ZOOM_MAX;
  }
  for (let i = ZOOM_STOPS.length - 1; i >= 0; i--) {
    const stop = ZOOM_STOPS[i];
    if (stop < now - EPS) return stop;
  }
  return ZOOM_MIN;
}

/** The root font size in CSS pixels for a zoom level. */
export function rootFontSize(zoom: number): number {
  return ZOOM_BASE_PX * clampZoom(zoom);
}

export function zoomPercent(zoom: number): number {
  return Math.round(clampZoom(zoom) * 100);
}

export function formatZoom(zoom: number): string {
  return `${zoomPercent(zoom)}%`;
}

export function isDefaultZoom(zoom: number): boolean {
  return Math.abs(clampZoom(zoom) - ZOOM_DEFAULT) < EPS;
}

/** Only the shape of a KeyboardEvent this cares about, so the matcher can be
 *  checked without a DOM. */
export interface ZoomKeyEvent {
  key: string;
  code?: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
}

// Both the main row and the numpad, and both what the key *says* and where it
// *is*:
//
//  - an unshifted "+" reports key "=", which is why Ctrl+= has to zoom in;
//  - Ctrl+Shift+= reports "+", and people press it because the legend says +;
//  - the numpad reports "Add"/"Subtract" in older engines and "+"/"-" in
//    current ones, but its `code` is stable;
//  - with NumLock off, Numpad0 reports key "Insert", so the code is the only
//    thing left to match on.
const IN_KEYS = new Set(["=", "+", "Add"]);
const IN_CODES = new Set(["Equal", "NumpadAdd"]);
const OUT_KEYS = new Set(["-", "_", "Subtract"]);
const OUT_CODES = new Set(["Minus", "NumpadSubtract"]);
const RESET_KEYS = new Set(["0"]);
const RESET_CODES = new Set(["Digit0", "Numpad0"]);

/** Which zoom command a keystroke is, or null.
 *
 *  Ctrl or Cmd is required and Alt disqualifies: Ctrl+Alt+- is a layout's dead
 *  key on some keyboards, not a zoom. Shift is allowed, because shift is how
 *  half the world types "+". */
export function zoomKeyCommand(e: ZoomKeyEvent): ZoomCommand | null {
  if (!(e.ctrlKey || e.metaKey)) return null;
  if (e.altKey) return null;
  const code = e.code ?? "";
  if (IN_KEYS.has(e.key) || IN_CODES.has(code)) return "in";
  if (OUT_KEYS.has(e.key) || OUT_CODES.has(code)) return "out";
  if (RESET_KEYS.has(e.key) || RESET_CODES.has(code)) return "reset";
  return null;
}

export interface ZoomWheelEvent {
  deltaY: number;
  ctrlKey?: boolean;
  metaKey?: boolean;
}

/** Ctrl+wheel, the other reflex. Only the sign is used, for the same reason
 *  the volume wheel only uses the sign. */
export function zoomWheelCommand(e: ZoomWheelEvent): ZoomCommand | null {
  if (!(e.ctrlKey || e.metaKey)) return null;
  if (e.deltaY < 0) return "in";
  if (e.deltaY > 0) return "out";
  return null;
}

/** What local storage held, made safe. Anything unparseable is 100% — a
 *  corrupt preference must not leave the app unreadable at boot. */
export function parseZoom(raw: string | null | undefined): number {
  if (raw == null) return ZOOM_DEFAULT;
  const n = Number(raw);
  if (!Number.isFinite(n) || n <= 0) return ZOOM_DEFAULT;
  return clampZoom(n);
}
