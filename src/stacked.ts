// Remembering the stacked layout, the side-effecting half. The predicate and
// the parsing are pure and live in src/logic/stack.ts.
//
// Local storage rather than SettingsDto, for the same reason as zoom: which
// shape a window wants is a property of this machine's screen, not of the
// account. A portrait monitor stacks; a laptop mostly cannot.

import { STACK_STORAGE_KEY, parseStacked } from "./logic/stack";

/** Storage throws outright in a few configurations — a WebView with storage
 *  disabled, private mode in some engines — and a layout preference is never
 *  worth taking the app down with it. */
function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function loadStacked(): boolean {
  try {
    return parseStacked(storage()?.getItem(STACK_STORAGE_KEY) ?? null);
  } catch {
    return false;
  }
}

export function saveStacked(on: boolean): void {
  try {
    storage()?.setItem(STACK_STORAGE_KEY, on ? "1" : "0");
  } catch {
    // Remembering failed; the layout still changed. Nothing to say about it.
  }
}
