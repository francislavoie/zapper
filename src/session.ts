// Remembering where you were, the side-effecting half. Parsing is pure and
// lives in src/logic/session.ts.
//
// Writes are coalesced. Scroll positions change on every wheel notch and a
// write per notch would be hundreds of JSON serialisations a second for a
// value only ever read once, at startup. A short trailing delay collapses a
// whole scroll gesture into one write, and the page-hide handler flushes so
// that closing mid-scroll still remembers where you were.

import { EMPTY_SESSION, SESSION_STORAGE_KEY, parseSession, serialiseSession, type Session } from "./logic/session";

/** Storage throws outright in a few configurations — a WebView with storage
 *  disabled, private mode in some engines — and remembering a scroll offset
 *  is never worth taking the app down with it. */
function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

let current: Session | null = null;
let timer: number | undefined;

export function loadSession(): Session {
  if (current) return current;
  try {
    current = parseSession(storage()?.getItem(SESSION_STORAGE_KEY) ?? null);
  } catch {
    current = { ...EMPTY_SESSION };
  }
  return current;
}

function flush(): void {
  if (timer !== undefined) {
    clearTimeout(timer);
    timer = undefined;
  }
  if (!current) return;
  try {
    storage()?.setItem(SESSION_STORAGE_KEY, serialiseSession(current));
  } catch {
    // Out of quota, or storage disabled. The app is unaffected; only the
    // next startup is, and only by starting at the top.
  }
}

/** How long a burst of changes is collected before one write. Long enough to
 *  swallow a scroll gesture, short enough that a crash loses nothing anyone
 *  would notice. */
const COALESCE_MS = 400;

export function saveSession(patch: Partial<Session>): void {
  current = { ...loadSession(), ...patch };
  if (typeof window === "undefined") {
    flush();
    return;
  }
  if (timer !== undefined) clearTimeout(timer);
  timer = window.setTimeout(flush, COALESCE_MS);
}

/** Registered once, so a window closed mid-scroll still remembers. `pagehide`
 *  rather than `beforeunload`: it fires on the paths a desktop webview
 *  actually takes, and does not ask to block the close. */
export function watchForExit(): void {
  if (typeof window === "undefined") return;
  window.addEventListener("pagehide", flush);
  // A window hidden to the tray may not come back before it is killed.
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") flush();
  });
}
