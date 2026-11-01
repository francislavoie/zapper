// The app's own context menu: the surface, its placement and its dismissal.
// What is in it belongs to whoever opens it — FavouriteMenu over a favourite,
// the category rail over a category, and the rail's options under its ▾.
//
// WebKit's browser menu is suppressed at the GTK level, so a right-click in
// this application produces nothing at all unless something draws it. This is
// that something: the same surface as the toolbar's menu, placed at the
// pointer and kept inside the window.
//
// Escape is not handled here. It belongs to the one place that decides what a
// single press unwinds — see useGlobalKeys — and an open context menu is first
// in that order. The arrow keys are handled here: they only ever mean "the
// next item in this menu".

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { documentScale } from "../zoom";
import { menuPosition } from "../logic/favourites";

export interface ContextMenuProps {
  /** Where to open, in visual (client) pixels. */
  x: number;
  y: number;
  /** What the menu is about, as the small heading at its top. */
  heading: string;
  onClose: () => void;
  /** Opened from the keyboard: take focus on the first item, and give it
   *  back to whatever had it when the menu goes. A pointer-opened menu leaves
   *  focus alone, as it always has. */
  takeFocus?: boolean;
  /** The button that opened it, if one did. A press on it is not a press
   *  "outside": it is left to the button, which closes the menu itself —
   *  otherwise the press closes it and the click that follows reopens it. */
  anchor?: RefObject<HTMLElement | null>;
  children: ReactNode;
}

/** Every kind of item: plain, checkbox and radio. */
const ITEM = '[role="menuitem"], [role="menuitemcheckbox"], [role="menuitemradio"]';

export function ContextMenu({
  x,
  y,
  heading,
  onClose,
  takeFocus,
  anchor,
  children,
}: ContextMenuProps) {
  const ref = useRef<HTMLDivElement | null>(null);
  // Placed off the measurement rather than off a guessed size: the menu's
  // width is its text's, and its text is a name from the catalogue.
  const [box, setBox] = useState(() => ({ left: x, top: y }));

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
    const placed = menuPosition(x, y, r.width, r.height, window.innerWidth, window.innerHeight);
    setBox({ left: placed.left / scale, top: placed.top / scale });
  }, [x, y, heading]);

  useEffect(() => {
    if (!takeFocus) return;
    const before = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>(ITEM)?.focus();
    return () => {
      // Only if it is still there: the row the menu was opened from may have
      // been windowed out, or moved, by what the menu just did.
      if (before?.isConnected) before.focus();
    };
  }, [takeFocus]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const target = e.target as Node;
      if (ref.current?.contains(target)) return;
      if (e.type === "mousedown" && anchor?.current?.contains(target)) return;
      onClose();
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
  }, [onClose, anchor]);

  /** Up and down step through the items and wrap; Home and End jump. */
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = { ArrowDown: 1, ArrowUp: -1, Home: 0, End: 0 }[e.key];
    if (step === undefined) return;
    const items = [...(ref.current?.querySelectorAll<HTMLElement>(ITEM) ?? [])];
    if (items.length === 0) return;
    e.preventDefault();
    const at = items.indexOf(document.activeElement as HTMLElement);
    const next =
      e.key === "Home"
        ? 0
        : e.key === "End"
          ? items.length - 1
          : at < 0
            ? step > 0
              ? 0
              : items.length - 1
            : (at + step + items.length) % items.length;
    items[next]?.focus();
  };

  return (
    <div
      className="ctx-menu"
      role="menu"
      aria-label={`${heading} options`}
      ref={ref}
      style={{ left: box.left, top: box.top }}
      onKeyDown={onKeyDown}
      // A right-click on the menu itself is not a request for another one.
      onContextMenu={(e) => e.preventDefault()}
    >
      <div className="ctx-head" data-tip-overflow="">
        {heading}
      </div>
      {children}
    </div>
  );
}
