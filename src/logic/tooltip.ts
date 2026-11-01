// Where a tooltip goes, as arithmetic.
//
// Kept apart from the DOM so the awkward cases — a window narrower than the
// tip, a control flush against the bottom edge, a tip taller than the room on
// either side — can be checked without a browser. Everything here is in one
// unit (the caller measures in visual pixels and converts once, on the way
// back into CSS; see documentScale in src/zoom.ts).

export interface TipTarget {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface TipPlacement {
  left: number;
  top: number;
  /** Which side of the target it ended up on, for a caller that wants to
   *  point an arrow at it or just to know. */
  side: "below" | "above";
}

/** Space between the target and the tip. Enough that the tip reads as
 *  belonging to the thing rather than as covering it. */
export const TIP_GAP = 6;

/** The closest a tip may come to the window's edge. The outer few pixels of
 *  an undecorated window are the resize grab, and a tip sitting on them looks
 *  clipped even when it is not. */
export const TIP_MARGIN = 6;

/** Below the target, like the native tooltip it replaces, unless there is no
 *  room there — then above, and failing both, whichever side has more room,
 *  clamped so the tip is on screen even if it has to overlap its target.
 *  Horizontally it centres on the target and slides to stay inside.
 *
 *  Below first because the targets that most need a tip are the ones along
 *  the top: the title bar's icon buttons. The player's bar is along the
 *  bottom, and there the first choice simply does not fit and the flip does
 *  the right thing without anybody having to say so. */
export function placeTip(
  target: TipTarget,
  width: number,
  height: number,
  viewportWidth: number,
  viewportHeight: number,
  gap = TIP_GAP,
  margin = TIP_MARGIN,
): TipPlacement {
  const below = target.bottom + gap;
  const above = target.top - gap - height;
  const lowest = viewportHeight - margin - height;

  let top: number;
  let side: TipPlacement["side"];
  if (below <= lowest) {
    top = below;
    side = "below";
  } else if (above >= margin) {
    top = above;
    side = "above";
  } else {
    // Fits neither side cleanly: a tall tip in a short window. Take the
    // roomier side and let the clamp pull it onto the screen.
    const roomBelow = viewportHeight - target.bottom;
    const roomAbove = target.top;
    side = roomBelow >= roomAbove ? "below" : "above";
    top = side === "below" ? below : above;
  }
  top = clamp(top, margin, lowest);

  const centred = (target.left + target.right) / 2 - width / 2;
  const left = clamp(centred, margin, viewportWidth - margin - width);

  return { left, top, side };
}

/** Lower bound wins: a tip wider than the window starts at the margin and
 *  runs off the far side rather than starting off the near one, so its first
 *  words — the ones that matter — are always readable. */
function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(value, max));
}
