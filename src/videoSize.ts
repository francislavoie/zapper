// Remembering the docked picture's size, the side-effecting half. The
// arithmetic is pure and lives in src/logic/videoSize.ts.

import {
  VIDEO_WIDTH_STORAGE_KEY,
  clampVideoWidth,
  parseVideoWidth,
} from "./logic/videoSize";

/** Storage throws outright in a few configurations — a WebView with storage
 *  disabled, private mode in some engines — and a size preference is never
 *  worth taking the app down with it. */
function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function loadVideoWidth(): number {
  try {
    return parseVideoWidth(storage()?.getItem(VIDEO_WIDTH_STORAGE_KEY) ?? null);
  } catch {
    return parseVideoWidth(null);
  }
}

export function saveVideoWidth(rem: number): void {
  try {
    storage()?.setItem(VIDEO_WIDTH_STORAGE_KEY, String(clampVideoWidth(rem)));
  } catch {
    // Remembering failed; the picture still resized.
  }
}

/** Writes the one custom property the stylesheet sizes everything from.
 *
 *  The band's height, the browse grid's first row and the slot itself are all
 *  derived from `--video-w`, and the rect mpv is given is measured from the
 *  slot afterwards — so this is the only write needed. */
export function applyVideoWidth(rem: number): void {
  if (typeof document === "undefined") return;
  document.documentElement.style.setProperty("--video-w", `${clampVideoWidth(rem)}rem`);
}
