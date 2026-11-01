// Installs the page's own mouse-wheel stepping; see src/logic/wheel.ts.
//
// One listener on the window, in the bubble phase, so anything closer to the
// pointer that uses the wheel for itself — volume over the picture, the guide
// grid's own handling — has already run and called preventDefault, and is
// left alone. Not passive: taking the notch means cancelling WebKit's
// animated scroll.

import { useEffect } from "react";
import { stepPixels, takesWheel } from "../logic/wheel";

/** The nearest ancestor that can actually scroll vertically in the wheel's
 *  direction, or null to leave the event to the engine. */
function scrollerFor(start: EventTarget | null, dy: number): HTMLElement | null {
  let el = start instanceof Element ? start : null;
  while (el && el !== document.documentElement) {
    if (el instanceof HTMLElement && el.scrollHeight > el.clientHeight) {
      const oy = getComputedStyle(el).overflowY;
      if (oy === "auto" || oy === "scroll") {
        const atTop = el.scrollTop <= 0;
        const atBottom = el.scrollTop + el.clientHeight >= el.scrollHeight - 1;
        // A box already at its end passes the wheel outwards, as native
        // scrolling does.
        if ((dy < 0 && !atTop) || (dy > 0 && !atBottom)) return el;
      }
    }
    el = el.parentElement;
  }
  return null;
}

export function useWheelSteps(): void {
  useEffect(() => {
    const onWheel = (e: WheelEvent) => {
      if (!takesWheel(pick(e))) return;
      const box = scrollerFor(e.target, e.deltaY);
      if (!box) return;
      e.preventDefault();
      box.scrollTop += stepPixels(e, box.clientHeight);
    };
    window.addEventListener("wheel", onWheel, { passive: false });
    return () => window.removeEventListener("wheel", onWheel);
  }, []);
}

function pick(e: WheelEvent) {
  return {
    deltaX: e.deltaX,
    deltaY: e.deltaY,
    deltaMode: e.deltaMode,
    ctrlKey: e.ctrlKey,
    metaKey: e.metaKey,
    shiftKey: e.shiftKey,
    defaultPrevented: e.defaultPrevented,
  };
}
