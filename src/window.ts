// The window itself, as a handful of verbs.
//
// The window is undecorated — Rust sets `decorations: false` — so the title
// bar this app draws is the only one there is, and dragging, minimising,
// maximising and closing have to be asked for explicitly. On a Mac the window
// keeps its own buttons (crates/zapper/tauri.macos.conf.json) and the bar here
// only drags it and answers its double click. Under Tauri these
// are real window calls, each one already granted in the capabilities; in a
// plain browser they would throw, so the mock stands in and records what was
// asked, the same trick `__lastVideoRect` plays for the video plane.
//
// Deliberately not in `src/api.ts`: that file mirrors the Rust command
// boundary and these are not commands. It must not be edited, so the switch
// lives here, beside `backend.ts`, which does the same job for commands.

import { inTauri } from "./backend";
import { mockWindow } from "./mock";

/** An edge or corner of the window, in Tauri's own spelling. */
export type ResizeDirection =
  | "North"
  | "South"
  | "East"
  | "West"
  | "NorthEast"
  | "NorthWest"
  | "SouthEast"
  | "SouthWest";

export interface WindowControls {
  /** Hand the drag to the compositor. The mouse-up that ends it is delivered
   *  to the window manager, not to us, so no click follows it. */
  startDragging(): Promise<void>;
  /** The same hand-off, for resizing from an edge or corner. */
  startResizeDragging(direction: ResizeDirection): Promise<void>;
  minimise(): Promise<void>;
  toggleMaximise(): Promise<void>;
  isMaximised(): Promise<boolean>;
  close(): Promise<void>;
  /** A double click on the title bar, on a Mac: whatever System Settings
   *  says a double click there does (zoom, fill, minimise or nothing), which
   *  only Rust can read. Elsewhere the bar maximises, as it always has. */
  titleBarDoubleClick(): Promise<void>;
}

// Imported lazily so that a plain browser never loads the Tauri window module
// at all, and so the bundle for `vite dev` does not pull it in.
const win = async () => (await import("@tauri-apps/api/window")).getCurrentWindow();

const tauriWindow: WindowControls = {
  startDragging: async () => (await win()).startDragging(),
  startResizeDragging: async (direction) => (await win()).startResizeDragging(direction),
  minimise: async () => (await win()).minimize(),
  toggleMaximise: async () => (await win()).toggleMaximize(),
  isMaximised: async () => (await win()).isMaximized(),
  close: async () => (await win()).close(),
  titleBarDoubleClick: async () =>
    (await import("@tauri-apps/api/core")).invoke<void>("title_bar_double_click"),
};

export const windowControls: WindowControls = inTauri ? tauriWindow : mockWindow;
