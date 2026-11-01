// Application zoom, bound to the two gestures people already have in their
// fingers: Ctrl with + / - / 0 (⌘ on a Mac), and Ctrl with the wheel (which
// is also what a trackpad pinch sends).
//
// Both listeners are on the window and neither asks whether a text field has
// focus. That is deliberate — Ctrl+- while the cursor is in the search box
// still means "make this smaller", the same as it does in every browser and
// every Electron app.

import { useCallback, useEffect, useLayoutEffect, useRef, type Dispatch } from "react";
import { zoomKeyCommand, zoomWheelCommand, type ZoomCommand } from "../logic/zoom";
import { IS_MAC } from "../platform";
import type { Action } from "../store";
import { applyZoom, saveZoom } from "../zoom";

/** How close together two wheel zooms may be. A trackpad fires far faster than
 *  anyone means to zoom. */
const WHEEL_GATE_MS = 110;

/** Move one stop. Where that lands is the reducer's business: it holds the
 *  current zoom, and a caller that read it here would read a stale value the
 *  moment two events arrive before a render. */
export function stepZoom(dispatch: Dispatch<Action>, command: ZoomCommand): void {
  dispatch({ type: "zoomStep", command });
}

export interface ZoomControl {
  zoom: number;
  step: (command: ZoomCommand) => void;
  reset: () => void;
}

export function useZoom(zoom: number, dispatch: Dispatch<Action>): ZoomControl {
  const lastWheel = useRef(0);

  // Before paint, not after: a passive effect here would show one frame at the
  // old size on every change. It also has to land before the video rect is
  // measured, and every layout effect runs before every passive one — which is
  // exactly what useVideoRect relies on.
  useLayoutEffect(() => {
    applyZoom(zoom);
    saveZoom(zoom);
  }, [zoom]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const command = zoomKeyCommand(e, IS_MAC);
      if (!command) return;
      // Ctrl+- is the WebView's own zoom as well. Letting both happen would
      // scale the page on top of the interface, and the video rect — which is
      // window coordinates, not page coordinates — would no longer match.
      e.preventDefault();
      stepZoom(dispatch, command);
    };

    const onWheel = (e: WheelEvent) => {
      const command = zoomWheelCommand(e);
      if (!command) return;
      e.preventDefault();
      // A mouse notch is one event; a trackpad pinch is thirty in a frame, and
      // stepping on each of them would slam the interface to a limit before
      // the fingers had finished moving. One stop per WHEEL_GATE_MS, which is
      // still instant for a mouse.
      const t = performance.now();
      if (t - lastWheel.current < WHEEL_GATE_MS) return;
      lastWheel.current = t;
      stepZoom(dispatch, command);
    };

    window.addEventListener("keydown", onKey);
    // Not passive: preventing the browser's own pinch-zoom is the whole point,
    // and a passive listener is not allowed to.
    window.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("wheel", onWheel);
    };
  }, [dispatch]);

  const step = useCallback((command: ZoomCommand) => stepZoom(dispatch, command), [dispatch]);
  const reset = useCallback(() => stepZoom(dispatch, "reset"), [dispatch]);

  return { zoom, step, reset };
}
