// The right-click menu on the picture.
//
// WebKit's own context menu is suppressed at the GTK level — Back, Reload
// and Inspect Element mean nothing here, and "Reload" would throw away the
// running player. This is what takes its place over the video: a short list
// of things that are actually about the stream.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { menuPosition } from "../logic/favourites";
import { useDispatch } from "../store";
import { documentScale } from "../zoom";

export interface VideoMenuProps {
  at: { x: number; y: number };
  onClose: () => void;
}

export function VideoMenu({ at, onClose }: VideoMenuProps) {
  const dispatch = useDispatch();
  const ref = useRef<HTMLDivElement | null>(null);
  const [box, setBox] = useState(() => ({ left: at.x, top: at.y }));

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    // Measured in visual pixels and written back as a CSS length, which is
    // not the same unit under CSS zoom. See `documentScale`.
    const scale = documentScale();
    const placed = menuPosition(at.x, at.y, r.width, r.height, window.innerWidth, window.innerHeight);
    setBox({ left: placed.left / scale, top: placed.top / scale });
  }, [at.x, at.y]);

  useEffect(() => {
    const dismiss = (e: Event) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    // Capture, so a click elsewhere closes this before that click acts.
    window.addEventListener("mousedown", dismiss, true);
    window.addEventListener("contextmenu", dismiss, true);
    window.addEventListener("resize", onClose);
    return () => {
      window.removeEventListener("mousedown", dismiss, true);
      window.removeEventListener("contextmenu", dismiss, true);
      window.removeEventListener("resize", onClose);
    };
  }, [onClose]);

  return (
    <div
      className="ctx-menu"
      role="menu"
      aria-label="Stream options"
      ref={ref}
      style={{ left: box.left, top: box.top }}
    >
      <button
        className="ctx-item"
        role="menuitem"
        onClick={() => dispatch({ type: "stats", open: true })}
      >
        Stats for nerds
      </button>
    </div>
  );
}
