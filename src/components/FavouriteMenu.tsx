// The app's own context menu, for forgetting a favourite.
//
// WebKit's browser menu is suppressed at the GTK level, so a right-click in
// this application produces nothing at all unless something draws it. This is
// that something: the same surface as the toolbar's menu, placed at the
// pointer and kept inside the window.
//
// Escape is not handled here. It belongs to the one place that decides what a
// single press unwinds — see useGlobalKeys — and this menu is first in that
// order while it is open.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { documentScale } from "../zoom";
import { menuPosition } from "../logic/favourites";
import type { FavouriteMenu as FavouriteMenuState } from "../store";

export interface FavouriteMenuProps {
  menu: FavouriteMenuState;
  onForget: () => void;
  onClose: () => void;
}

export function FavouriteMenu({ menu, onForget, onClose }: FavouriteMenuProps) {
  const ref = useRef<HTMLDivElement | null>(null);
  // Placed off the measurement rather than off a guessed size: the menu's
  // width is its text's, and its text is a channel name.
  const [box, setBox] = useState(() => ({ left: menu.x, top: menu.y }));

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    // All of this is measured in visual pixels — the pointer, the menu's own
    // box, the window. The answer is written back as a CSS length, which is
    // not the same unit under CSS zoom, so it is converted once here. Without
    // it the menu lands at pointer × zoom: right at the top-left corner and
    // further out the further across the window you click.
    const scale = documentScale();
    const placed = menuPosition(
      menu.x,
      menu.y,
      r.width,
      r.height,
      window.innerWidth,
      window.innerHeight,
    );
    setBox({ left: placed.left / scale, top: placed.top / scale });
  }, [menu.x, menu.y, menu.streamId]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    // Capture, so a click elsewhere closes this before that click acts rather
    // than after it.
    window.addEventListener("mousedown", onDown, true);
    // A second right-click, anywhere, replaces or dismisses the menu; it must
    // not leave two of them on screen.
    window.addEventListener("contextmenu", onDown, true);
    window.addEventListener("resize", onClose);
    return () => {
      window.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("contextmenu", onDown, true);
      window.removeEventListener("resize", onClose);
    };
  }, [onClose]);

  return (
    <div
      className="ctx-menu"
      role="menu"
      aria-label={`${menu.name} options`}
      ref={ref}
      style={{ left: box.left, top: box.top }}
    >
      <div className="ctx-head">{menu.name}</div>
      <button className="tb-menu-item" role="menuitem" onClick={onForget}>
        <span className="tb-menu-label">Forget this channel</span>
      </button>
      <div className="tb-menu-foot">
        Clears its watch history, so it stops crowding the list.
      </div>
    </div>
  );
}
