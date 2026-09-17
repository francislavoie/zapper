// Remembering the rail's width, the side-effecting half. The arithmetic is
// pure and lives in src/logic/railWidth.ts.

import {
  RAIL_WIDTH_STORAGE_KEY,
  clampRailWidth,
  parseRailWidth,
} from "./logic/railWidth";

function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function loadRailWidth(): number {
  try {
    return parseRailWidth(storage()?.getItem(RAIL_WIDTH_STORAGE_KEY) ?? null);
  } catch {
    return parseRailWidth(null);
  }
}

export function saveRailWidth(rem: number): void {
  try {
    storage()?.setItem(RAIL_WIDTH_STORAGE_KEY, String(clampRailWidth(rem)));
  } catch {
    // Remembering failed; the rail still resized.
  }
}

/** Writes the custom property the browse grid's first column is sized from. */
export function applyRailWidth(rem: number): void {
  if (typeof document === "undefined") return;
  document.documentElement.style.setProperty("--rail-w", `${clampRailWidth(rem)}rem`);
}
