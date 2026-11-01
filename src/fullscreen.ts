// Fullscreen is a window property, not a DOM one: the video is drawn by mpv
// beneath the webview, so making a DOM element fullscreen would achieve
// nothing in the real app. Under Tauri this asks the window; in the browser
// mock it falls back to the document API so the layout can still be seen at
// another size.

import { inTauri } from "./backend";

export async function isFullscreen(): Promise<boolean> {
  if (!inTauri) return document.fullscreenElement !== null;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow().isFullscreen();
}

/** Leaves fullscreen, and only that. Not the toggle: whatever the cached
 *  state says, Escape must never be the key that *enters* fullscreen. */
export async function exitFullscreen(): Promise<void> {
  if (!inTauri) {
    if (document.fullscreenElement) await document.exitFullscreen();
    return;
  }
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().setFullscreen(false);
}

export async function toggleFullscreen(): Promise<boolean> {
  if (!inTauri) {
    if (document.fullscreenElement) {
      await document.exitFullscreen();
      return false;
    }
    await document.documentElement.requestFullscreen();
    return true;
  }
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const w = getCurrentWindow();
  const next = !(await w.isFullscreen());
  await w.setFullscreen(next);
  return next;
}
