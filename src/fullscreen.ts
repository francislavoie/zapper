// Fullscreen is the document's: the page asks the browser to show it full
// screen.

export async function isFullscreen(): Promise<boolean> {
  return document.fullscreenElement !== null;
}

/** Leaves fullscreen, and only that. Not the toggle: whatever the cached
 *  state says, Escape must never be the key that *enters* fullscreen. */
export async function exitFullscreen(): Promise<void> {
  if (document.fullscreenElement) await document.exitFullscreen();
}

export async function toggleFullscreen(): Promise<boolean> {
  if (document.fullscreenElement) {
    await document.exitFullscreen();
    return false;
  }
  await document.documentElement.requestFullscreen();
  return true;
}
