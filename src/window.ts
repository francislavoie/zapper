// The window itself, as a handful of verbs.
//
// A browser has no window to drag, minimise or close, so the mock stands in
// and records what was asked, the same trick `__lastVideoRect` plays for the
// video plane.

import { mockWindow } from "./mock";

/** An edge or corner of the window, in the window manager's spelling. */
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

export const windowControls: WindowControls = mockWindow;
