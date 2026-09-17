// The left rail, and the tab bar above it.
//
// Two lists live here and only one is on screen: the categories, which are
// the catalogue's own structure, and the favourites, which are this
// installation's. They share the column because they answer the same question
// — what do I want to watch — from opposite ends, and because the rail is the
// only place in the layout where a second list costs nothing.
//
// Both are always mounted, inside <Activity>. Unmounting the pane being left
// would throw away everything it had: the categories would be re-fetched and
// the rail would say "Loading categories" every single time the tab was
// clicked, and the scroll position, the filter text and the favourites list
// would go with it. Activity keeps a hidden tree's state and its DOM while
// unmounting its effects, so coming back is instant and nothing re-fetches
// that does not need to.
//
// The switch itself is a transition. It is not urgent — the tab highlights
// immediately either way — and marking it keeps a long list rendering from
// blocking the click.

import { Activity, useRef, useTransition, type PointerEvent as ReactPointerEvent } from "react";
import { CategoryRail } from "./CategoryRail";
import { FavouritesRail } from "./FavouritesRail";
import {
  RAIL_WIDTH_DEFAULT,
  isDefaultRailWidth,
  railAfterDrag,
} from "../logic/railWidth";
import { useApp, useDispatch, type RailTab } from "../store";

const TABS: Array<{ id: RailTab; label: string }> = [
  { id: "categories", label: "Categories" },
  { id: "favourites", label: "Favourites" },
];

export function Rail() {
  const { railTab, railWidthRem } = useApp();
  const dispatch = useDispatch();
  const [switching, startSwitch] = useTransition();
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
    <aside className={`rail ${switching ? "switching" : ""}`} ref={railRef}>
      <div className="rail-tabs" role="tablist" aria-label="Channel lists">
        {TABS.map((t) => (
          <button
            key={t.id}
            className={`rail-tab ${railTab === t.id ? "sel" : ""}`}
            role="tab"
            aria-selected={railTab === t.id}
            onClick={() => startSwitch(() => dispatch({ type: "railTab", tab: t.id }))}
          >
            {t.label}
          </button>
        ))}
      </div>

      {/* `mode` is the whole switch. A hidden Activity renders nothing and
          costs nothing to keep, and its subtree comes back exactly as it was
          left. */}
      <Activity mode={railTab === "categories" ? "visible" : "hidden"}>
        <CategoryRail />
      </Activity>
      <Activity mode={railTab === "favourites" ? "visible" : "hidden"}>
        <FavouritesRail />
      </Activity>
      {/* On the boundary the two panes share. A separator rather than a
          button: it sets a value along an axis. */}
      <span
        className="rail-grip"
        role="separator"
        aria-label="Rail width"
        aria-orientation="vertical"
        aria-valuenow={Math.round(railWidthRem * 4) / 4}
        tabIndex={0}
        title={
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
