// The one tooltip. Mounted once, at the top of the app; every target in the
// page points at it by attribute — see src/tooltip.ts for the whole scheme
// and why it is attributes rather than a component per target.
//
// In a portal on <body>, outside `.app`, so no stacking context in the app
// can put anything over it, and no `overflow: hidden` pane can clip it. It
// never takes the pointer: the window's resize edges sit at the very top of
// the app's own order, and a tip drawn over one must not stop the edge from
// being grabbed through it.
//
// The element is driven directly rather than through React state. Showing it
// is a text change and two style writes, done in the same event that asked
// for it, so there is nothing for a render to add but a frame of delay.

import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { TIP_ID, installTooltips } from "../tooltip";

export function TooltipHost() {
  const ref = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const tip = ref.current;
    if (!tip) return;
    return installTooltips(tip);
  }, []);

  return createPortal(
    <div className="tip-layer">
      <div className="tip" id={TIP_ID} role="tooltip" ref={ref} hidden />
    </div>,
    document.body,
  );
}
