// The window itself, as five verbs.
//
// The window is undecorated — Rust sets `decorations: false` — so the title
// bar this app draws is the only one there is, and dragging, minimising,
// maximising and closing have to be asked for explicitly. Under Tauri these
// are real window calls, each one already granted in the capabilities; in a
// plain browser they would throw, so the mock stands in and records what was
// asked, the same trick `__lastVideoRect` plays for the video plane.
//
// Deliberately not in `src/api.ts`: that file mirrors the Rust command
// boundary and these are not commands. It must not be edited, so the switch
// lives here, beside `backend.ts`, which does the same job for commands.

import { inTauri } from "./backend";
import { mockWindow } from "./mock";

export interface WindowControls {
  /** Hand the drag to the compositor. The mouse-up that ends it is delivered
   *  to the window manager, not to us, so no click follows it. */
  startDragging(): Promise<void>;
  minimise(): Promise<void>;
  toggleMaximise(): Promise<void>;
  isMaximised(): Promise<boolean>;
  close(): Promise<void>;
}

// Imported lazily so that a plain browser never loads the Tauri window module
// at all, and so the bundle for `vite dev` does not pull it in.
const win = async () => (await import("@tauri-apps/api/window")).getCurrentWindow();

const tauriWindow: WindowControls = {
  startDragging: async () => (await win()).startDragging(),
  minimise: async () => (await win()).minimize(),
  toggleMaximise: async () => (await win()).toggleMaximize(),
  isMaximised: async () => (await win()).isMaximized(),
  close: async () => (await win()).close(),
};

export const windowControls: WindowControls = inTauri ? tauriWindow : mockWindow;
