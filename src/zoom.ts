// The side-effecting half of application zoom: the one DOM write that applies
// it, and the local-storage read and write that remember it. The arithmetic
// and the key matching are pure and live in src/logic/zoom.ts.
//
// Zoom is per machine, not per account: a 4K laptop and a TV across the room
// want different sizes from the same install, so this is local storage rather
// than SettingsDto, which is the Rust-side contract and is synced.

import { ZOOM_STORAGE_KEY, clampZoom, parseZoom } from "./logic/zoom";

/** Local storage throws outright in a few configurations — a WebView with
 *  storage disabled, private mode in some engines — and a display preference
 *  is never worth taking the app down with it. */
function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function loadZoom(): number {
  try {
    return parseZoom(storage()?.getItem(ZOOM_STORAGE_KEY) ?? null);
  } catch {
    return parseZoom(null);
  }
}

export function saveZoom(zoom: number): void {
  try {
    storage()?.setItem(ZOOM_STORAGE_KEY, String(clampZoom(zoom)));
  } catch {
    // Remembering failed; zooming still worked. Nothing to say about it.
  }
}

/** Reveals the interface, which index.html hides until zoom has settled.
 *  Idempotent, and safe to call when the class was already removed by the
 *  bootstrap's safety timer. */
function reveal(): void {
  document.documentElement.classList.remove("zoom-pending");
}

/** Applies zoom as real page zoom, not by scaling the root font size.
 *
 *  Scaling `font-size` only moves what is expressed in `rem`. The channel list
 *  and the guide position their rows absolutely from fixed pixel constants
 *  (`ROW_HEIGHT`, `GUIDE_ROW_HEIGHT`) because that is what virtualisation
 *  needs, so the text inside a row grew while the row itself stayed 26px tall.
 *  Page zoom scales the CSS pixel instead, so those constants scale with
 *  everything else and the JS arithmetic needs no special case at all.
 *
 *  Under Tauri this is WebKit's own zoom, the same mechanism the browser's
 *  Ctrl+= drives. In a plain browser - the mock, and the automated checks -
 *  there is no such API, so the CSS `zoom` property stands in; it scales
 *  layout the same way, which `font-size` never did.
 *
 *  Both paths scale `clientWidth`/`clientHeight` along with element boxes, so
 *  the normalised video rect stays correct without knowing the zoom at all.
 */
export function applyZoom(zoom: number): void {
  if (typeof document === "undefined") return;
  const scale = clampZoom(zoom);
  const root = document.documentElement;

  // Kept for anything that wants to react to the level in CSS. It does not
  // drive sizing.
  root.style.setProperty("--zoom", String(scale));

  const tauri = (globalThis as { __TAURI_INTERNALS__?: unknown })
    .__TAURI_INTERNALS__;
  if (!tauri) {
    root.style.zoom = String(scale);
    reveal();
    return;
  }

  // index.html has already set CSS zoom to the remembered value, so the page
  // is the right size before this resolves. Handing over to WebKit's own page
  // zoom means clearing that first write, and the two must never both be in
  // effect - they multiply. The swap therefore happens after `setZoom` has
  // taken, and while the interface is still hidden, so neither the doubled
  // size nor the bare 100% is ever painted.
  void import("@tauri-apps/api/webview")
    .then(({ getCurrentWebview }) => getCurrentWebview().setZoom(scale))
    .then(() => {
      root.style.zoom = "";
    })
    .catch(() => {
      // Permission missing or the call failed. CSS zoom is already applied
      // and correct; leaving it in place is the fallback.
      root.style.zoom = String(scale);
    })
    .finally(reveal);
}

/** How many visual pixels the document draws per CSS pixel.
 *
 *  Pointer coordinates (`clientX`, `getBoundingClientRect`, `innerWidth`) are
 *  all in visual pixels. A CSS length written back into the page is not: under
 *  CSS zoom it is multiplied by the zoom on the way to the screen. Position
 *  something at a measured `clientX` and it lands at `clientX × zoom`, so the
 *  error is zero at the origin and grows across the window — which is why it
 *  reads as "the position is scaled too" rather than as a constant offset.
 *
 *  `clientWidth` on the root reports the viewport and does not move with CSS
 *  zoom, but `offsetWidth` is the laid-out width and does, so their ratio is
 *  the factor. Under WebKit's page zoom nothing diverges and this is 1, so
 *  dividing by it is free and callers need not know which zoom is in play. */
export function documentScale(): number {
  if (typeof document === "undefined") return 1;
  const root = document.documentElement;
  const width = root.getBoundingClientRect().width;
  return root.offsetWidth > 0 && width > 0 ? width / root.offsetWidth : 1;
}
