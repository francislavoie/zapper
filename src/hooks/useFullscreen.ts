// Whether the window is fullscreen, as one value every component shares.
//
// There is no DOM event for this. Under Tauri fullscreen is a *window*
// property — the video is drawn by mpv beneath the webview, so making a DOM
// element fullscreen would achieve nothing — and asking the window to go
// fullscreen does not fire `fullscreenchange`. The player bar listened for
// that event and so never learned: its button went on offering "Fullscreen"
// while the app already was, and nothing else in the interface could react at
// all.
//
// The signal that does arrive is a resize, because a window changing to or
// from fullscreen changes size. So a resize re-asks, and the answer is kept
// in one external store rather than in each component's state: two copies of
// this would be two answers, and the one driving the layout and the one
// driving the icon have to agree.

import { useSyncExternalStore } from "react";
import { isFullscreen } from "../fullscreen";

let current = false;
let started = false;
const listeners = new Set<() => void>();

function publish(next: boolean): void {
  if (next === current) return;
  current = next;
  for (const l of listeners) l();
}

function refresh(): void {
  void isFullscreen()
    .then(publish)
    // Nothing to do about it and nothing worth saying: the value stays as it
    // was, which is the last answer the window actually gave.
    .catch(() => {});
}

/** Attaches the listeners once, the first time anybody asks. */
function start(): void {
  if (started || typeof window === "undefined") return;
  started = true;
  // The browser mock uses the document API, which does fire this.
  document.addEventListener("fullscreenchange", refresh);
  // Tauri's does not, but the window changes size, which is observable.
  window.addEventListener("resize", refresh);
  refresh();
}

function subscribe(onChange: () => void): () => void {
  start();
  listeners.add(onChange);
  return () => listeners.delete(onChange);
}

/** Re-asks the window immediately. Call after toggling, so the interface
 *  turns over on the click rather than on the resize that follows it. */
export function refreshFullscreen(): void {
  refresh();
}

/** The shared answer, read outside React — for a key handler that has to
 *  decide on the keystroke, not on the next render. */
export function fullscreenNow(): boolean {
  start();
  return current;
}

export function useFullscreen(): boolean {
  return useSyncExternalStore(
    subscribe,
    () => current,
    // Server/prerender: there is no window to be fullscreen.
    () => false,
  );
}
