// The left rail: one list of the rail's own items, the pinned categories and
// the rest (CategoryRail), and the grip that sizes the column.
//
// Favourites used to be a second pane behind a tab. They are an item in the
// list now, shown in the channel list like any category, so there is nothing
// to switch and nothing to keep alive behind a hidden tree.

import { useRef, type PointerEvent as ReactPointerEvent } from "react";
import { CategoryRail } from "./CategoryRail";
import {
  RAIL_WIDTH_DEFAULT,
  isDefaultRailWidth,
  railAfterDrag,
} from "../logic/railWidth";
import { useApp, useDispatch } from "../store";
import { RailFold } from "./icons";

/** Folds the rail away or brings it back: the first thing in the heading row
 *  of whichever view is showing, at the left edge of the content, so the
 *  button is where the rail's edge was either way. */
export function RailToggle() {
  const { railCollapsed } = useApp();
  const dispatch = useDispatch();
  const label = railCollapsed ? "Show the rail" : "Hide the rail";
  return (
    <button
      className="rail-fold"
      aria-label={label}
      data-tip={label}
      onClick={() => dispatch({ type: "railCollapsed", collapsed: !railCollapsed })}
    >
      <RailFold className="rail-fold-i" open={!railCollapsed} />
    </button>
  );
}

export function Rail() {
  const { railWidthRem } = useApp();
  const dispatch = useDispatch();
  const railRef = useRef<HTMLElement | null>(null);

  // Dragging the rail's right edge divides the space between it and the
  // channel list. The rail is pinned left and grows rightwards, so a
  // positive drag makes it wider — the opposite of the picture's handle,
  // which is pinned right.
  //
  // `pxPerRem` is measured from the rail rather than assumed to be 16: under
  // CSS zoom a rem and a client pixel are different sizes.
  const drag = useRef<{ x: number; rem: number; pxPerRem: number } | null>(null);

  const onGripDown = (e: ReactPointerEvent<HTMLElement>) => {
    const el = railRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    if (!(rect.width > 0) || !(railWidthRem > 0)) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { x: e.clientX, rem: railWidthRem, pxPerRem: rect.width / railWidthRem };
  };

  const onGripMove = (e: ReactPointerEvent<HTMLElement>) => {
    const d = drag.current;
    if (!d) return;
    dispatch({ type: "railWidth", rem: railAfterDrag(d.rem, e.clientX - d.x, d.pxPerRem) });
  };

  const endGrip = (e: ReactPointerEvent<HTMLElement>) => {
    if (!drag.current) return;
    drag.current = null;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
  };

  return (
    <aside className="rail" ref={railRef}>
      <CategoryRail />
      {/* On the boundary the rail shares with the list. A separator rather than a
          button: it sets a value along an axis. */}
      <span
        className="rail-grip"
        role="separator"
        aria-label="Rail width"
        aria-orientation="vertical"
        aria-valuenow={Math.round(railWidthRem * 4) / 4}
        tabIndex={0}
        data-tip={
          isDefaultRailWidth(railWidthRem)
            ? "Drag to resize the rail"
            : "Drag to resize · double-click to reset"
        }
        onPointerDown={onGripDown}
        onPointerMove={onGripMove}
        onPointerUp={endGrip}
        onPointerCancel={endGrip}
        onDoubleClick={() => dispatch({ type: "railWidth", rem: RAIL_WIDTH_DEFAULT })}
        onKeyDown={(e) => {
          const step = e.shiftKey ? 4 : 1;
          if (e.key === "ArrowRight") {
            e.preventDefault();
            dispatch({ type: "railWidth", rem: railWidthRem + step });
          } else if (e.key === "ArrowLeft") {
            e.preventDefault();
            dispatch({ type: "railWidth", rem: railWidthRem - step });
          } else if (e.key === "Home") {
            e.preventDefault();
            dispatch({ type: "railWidth", rem: RAIL_WIDTH_DEFAULT });
          }
        }}
      />
    </aside>
  );
}
