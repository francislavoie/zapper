// Invisible handles along the window's edges and corners, because the window
// is undecorated and nothing else reliably resizes it.
//
// Tauri does attach a native handler for this on Linux, but it is not enough
// here. It sits on the webview's button-press signal and only ever covers the
// outer 5px; WebKit owns the cursor, so the pointer never changes to say an
// edge is there; and under the overlay stack that puts the video beneath the
// page, a grab it starts does not reliably follow the pointer. Handles in the
// page fix all three: the cursor is CSS, the hit area is ours to size, and
// `startResizeDragging` hands the gesture to the compositor the same way the
// title bar hands it a move.
//
// Nothing interactive may sit under these strips — see rule 1 at the top of
// Toolbar.tsx, which keeps every control inside a padding ring for exactly
// this reason.

import type { ResizeDirection } from "../window";
import { windowControls } from "../window";
import { useMaximised } from "../hooks/useMaximised";

const EDGES: Array<[ResizeDirection, string]> = [
  ["North", "n"],
  ["South", "s"],
  ["East", "e"],
  ["West", "w"],
  ["NorthEast", "ne"],
  ["NorthWest", "nw"],
  ["SouthEast", "se"],
  ["SouthWest", "sw"],
];

export function ResizeEdges({ fullscreen }: { fullscreen: boolean }) {
  const maximised = useMaximised();
  // A maximised or fullscreen window has no edge to grab, and a strip of
  // resize cursor along the screen's border would only be in the way.
  if (maximised || fullscreen) return null;
  return (
    <div className="resize-edges" aria-hidden>
      {EDGES.map(([direction, cls]) => (
        <div
          key={cls}
          className={`resize-edge resize-${cls}`}
          onMouseDown={(e) => {
            if (e.button !== 0) return;
            e.preventDefault();
            e.stopPropagation();
            void windowControls.startResizeDragging(direction).catch(() => {
              // Nothing to say: the window simply stays the size it was.
            });
          }}
        />
      ))}
    </div>
  );
}
