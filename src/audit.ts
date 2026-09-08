// Two things about this UI cannot be judged by looking at it, and both are
// one-line failures:
//
//   - a light surface anywhere in a dark app;
//   - a painted pixel over the video plane, which mpv draws beneath the
//     webview, so anything opaque above it is a hole in the picture.
//
// Both are answerable by walking the DOM and reading computed styles, so this
// module answers them. It is wired to `globalThis` by the mock backend only —
// the same idea as `__lastVideoRect` — so a browser driver can assert instead
// of squinting, and the real app never reads it.

import { isLightSurface, paintsAnything } from "./logic/contrast";
import type { VideoRect } from "./types";

export interface Offender {
  selector: string;
  background: string;
  box: { x: number; y: number; w: number; h: number };
}

interface Edges {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

const describe = (el: Element): string => {
  const cls =
    typeof el.className === "string" && el.className.trim() !== ""
      ? `.${el.className.trim().split(/\s+/).join(".")}`
      : "";
  return `${el.tagName.toLowerCase()}${el.id ? `#${el.id}` : ""}${cls}`;
};

const asOffender = (el: Element, background: string, e: Edges): Offender => ({
  selector: describe(el),
  background,
  box: { x: e.left, y: e.top, w: e.right - e.left, h: e.bottom - e.top },
});

/** What of an element is actually on screen: its box, intersected with every
 *  scrolling or clipping ancestor and then with the window.
 *
 *  Without this the virtualized list looks like a catastrophe — its rows exist
 *  at spacer coordinates thousands of pixels outside the viewport that clips
 *  them, and half of them "overlap" the video. `null` means nothing of the
 *  element is visible. (Positioned descendants can escape a clipping ancestor
 *  that is not their containing block; nothing in this app does that, and a
 *  test hook may be a little conservative.) */
export function visibleBox(el: Element): Edges | null {
  const b = el.getBoundingClientRect();
  const box: Edges = { left: b.left, top: b.top, right: b.right, bottom: b.bottom };
  for (let p = el.parentElement; p; p = p.parentElement) {
    const cs = getComputedStyle(p);
    const clips =
      cs.overflow !== "visible" || cs.overflowX !== "visible" || cs.overflowY !== "visible";
    if (!clips) continue;
    const r = p.getBoundingClientRect();
    box.left = Math.max(box.left, r.left);
    box.top = Math.max(box.top, r.top);
    box.right = Math.min(box.right, r.right);
    box.bottom = Math.min(box.bottom, r.bottom);
    if (box.right <= box.left || box.bottom <= box.top) return null;
  }
  box.left = Math.max(box.left, 0);
  box.top = Math.max(box.top, 0);
  box.right = Math.min(box.right, window.innerWidth);
  box.bottom = Math.min(box.bottom, window.innerHeight);
  return box.right > box.left && box.bottom > box.top ? box : null;
}

const painting = (el: Element): { cs: CSSStyleDeclaration; box: Edges } | null => {
  const cs = getComputedStyle(el);
  if (cs.visibility === "hidden" || cs.display === "none" || Number(cs.opacity) === 0) {
    return null;
  }
  const box = visibleBox(el);
  return box ? { cs, box } : null;
};

/** Every element painting a light background bigger than `minSize` square.
 *  Should be empty: this app is dark everywhere, in every view. */
export function auditLightSurfaces(minSize = 24): Offender[] {
  const out: Offender[] = [];
  for (const el of Array.from(document.querySelectorAll("*"))) {
    const p = painting(el);
    if (!p) continue;
    if (p.box.right - p.box.left < minSize || p.box.bottom - p.box.top < minSize) continue;
    if (isLightSurface(p.cs.backgroundColor)) {
      out.push(asOffender(el, p.cs.backgroundColor, p.box));
    }
  }
  return out;
}

/** Every element that paints anything over the video rect. Should be empty in
 *  browse mode; in the mock, `.mock-video` itself is excluded because it *is*
 *  the stand-in for the video plane. */
export function auditOverVideo(rect: VideoRect | undefined): Offender[] {
  if (!rect || rect.w <= 0 || rect.h <= 0) return [];
  const left = rect.x * window.innerWidth;
  const top = rect.y * window.innerHeight;
  const right = left + rect.w * window.innerWidth;
  const bottom = top + rect.h * window.innerHeight;

  const out: Offender[] = [];
  for (const el of Array.from(document.querySelectorAll("*"))) {
    if (el.classList.contains("mock-video")) continue;
    const p = painting(el);
    if (!p) continue;
    const b = p.box;
    if (!(b.left < right && b.right > left && b.top < bottom && b.bottom > top)) continue;
    if (paintsAnything(p.cs.backgroundColor) || p.cs.backgroundImage !== "none") {
      out.push(asOffender(el, p.cs.backgroundColor, b));
    }
  }
  return out;
}
