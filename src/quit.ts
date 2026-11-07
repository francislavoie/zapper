// What the page does when the app quits.
//
// The desktop app does not navigate the page away when it quits: it asks the
// page to say goodbye through `window.__zapperQuit`, waits for the promise,
// and only then ends the page's process (src-tauri/src/surface/linux). So the
// goodbye has to cover everything that would otherwise be lost: `pagehide`
// for whatever saves on it (the session, a debounced settings change), and
// the saves already on their way to the backend, which are calls across the
// IPC bridge that the process must outlive.

/** Saves on their way to the backend. Added by whoever starts one, removed
 *  when it settles. */
const pending = new Set<Promise<unknown>>();

/** Tracks a save the quit must wait for; returns it, for chaining. */
export function trackSave<T>(save: Promise<T>): Promise<T> {
  pending.add(save);
  const done = () => pending.delete(save);
  save.then(done, done);
  return save;
}

declare global {
  interface Window {
    __zapperQuit?: () => Promise<void>;
  }
}

/** Installs `window.__zapperQuit`. Once, at startup. */
export function installQuitHook(): void {
  if (typeof window === "undefined") return;
  window.__zapperQuit = async () => {
    window.dispatchEvent(new PageTransitionEvent("pagehide", { persisted: false }));
    await Promise.allSettled([...pending]);
  };
}
