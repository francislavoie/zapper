// Whether the window currently has room for the stacked layout.
//
// A hook rather than a value read at render time because the answer changes
// when the window is resized and when the page is zoomed, and both of those
// have to reach the control that offers the layout — otherwise the chevron
// stays on screen after the room for it has gone.

import { useEffect, useState } from "react";
import { canStack } from "../logic/stack";
import { TOOLBAR_HEIGHT } from "../logic/toolbar";

/** The stage is everything below the app's own title bar. The bar's height is
 *  a constant in CSS pixels and does not scale with page zoom, so it is
 *  subtracted here rather than assumed inside the predicate. */
const measure = (): boolean =>
  typeof window === "undefined"
    ? false
    : canStack(
        window.innerWidth,
        window.innerHeight,
        window.innerHeight - TOOLBAR_HEIGHT,
      );

export function useCanStack(zoom?: number): boolean {
  const [room, setRoom] = useState(measure);

  useEffect(() => {
    const update = () => setRoom(measure());
    // Zoom scales the CSS pixel, so `innerWidth`/`innerHeight` change without
    // the window moving and without a resize event on every engine. Re-measure
    // when it changes as well.
    update();
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, [zoom]);

  return room;
}
