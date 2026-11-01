// Remembering the category rail's pins, hidden categories and sort, the
// side-effecting half. The ordering and the parsing are pure and live in
// src/logic/categoryOrder.ts.

import {
  CATEGORY_HIDDEN_STORAGE_KEY,
  CATEGORY_PINS_STORAGE_KEY,
  CATEGORY_SORT_STORAGE_KEY,
  parseHidden,
  parsePins,
  parseSort,
  serialiseHidden,
  serialisePins,
  type CategorySort,
} from "./logic/categoryOrder";

/** Storage throws outright in a few configurations — a WebView with storage
 *  disabled, private mode in some engines — and a list preference is never
 *  worth taking the app down with it. */
function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function loadCategoryPins(): string[] {
  try {
    return parsePins(storage()?.getItem(CATEGORY_PINS_STORAGE_KEY) ?? null);
  } catch {
    return [];
  }
}

export function saveCategoryPins(pins: readonly string[]): void {
  try {
    storage()?.setItem(CATEGORY_PINS_STORAGE_KEY, serialisePins(pins));
  } catch {
    // Remembering failed; the rail still reordered.
  }
}

export function loadCategorySort(): CategorySort {
  try {
    return parseSort(storage()?.getItem(CATEGORY_SORT_STORAGE_KEY) ?? null);
  } catch {
    return parseSort(null);
  }
}

export function saveCategorySort(sort: CategorySort): void {
  try {
    storage()?.setItem(CATEGORY_SORT_STORAGE_KEY, sort);
  } catch {
    // As above.
  }
}

export function loadCategoryHidden(): string[] {
  try {
    return parseHidden(storage()?.getItem(CATEGORY_HIDDEN_STORAGE_KEY) ?? null);
  } catch {
    return [];
  }
}

export function saveCategoryHidden(hidden: readonly string[]): void {
  try {
    storage()?.setItem(CATEGORY_HIDDEN_STORAGE_KEY, serialiseHidden(hidden));
  } catch {
    // As above.
  }
}
