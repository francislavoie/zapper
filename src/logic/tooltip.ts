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

/** Wider than this, a target is a row or a heading rather than a control,
 *  and its middle can be a long way from anything the viewer is looking at:
 *  the tip goes by the pointer, or by the text, instead (see tipAnchor). */
export const WIDE_TARGET = 240;

/** Where along the target the tip lines up, horizontally.
 *
 *  `start`: the tip's left edge at `x` — for a name cut short, where `x` is
 *  where the name begins, so the full text reads on from the same place.
 *  `centre`: the tip centred on `x` — the pointer, over a wide target.
 *  Without one, the tip centres on the target, which is right for a button. */
export interface TipAnchor {
  x: number;
  align: "start" | "centre";
}

/** Below the target, like the native tooltip it replaces, unless there is no
 *  room there — then above, and failing both, whichever side has more room,
 *  clamped so the tip is on screen even if it has to overlap its target.
 *  Horizontally it centres on the target, or lines up with `anchor` when
 *  there is one, and slides to stay inside.
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
  anchor: TipAnchor | null = null,
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

  const wanted =
    anchor === null
      ? (target.left + target.right) / 2 - width / 2
      : anchor.align === "start"
        ? anchor.x
        : anchor.x - width / 2;
  const left = clamp(wanted, margin, viewportWidth - margin - width);

  return { left, top, side };
}

/** Whether text is cut short, from what the DOM reports (src/tooltip.ts
 *  isTruncated measures; this decides).
 *
 *  `scroll` and `client` are the element's scroll and client sizes along
 *  the axis it can overflow on. Both are whole pixels, and WebKit can round
 *  a fractional box's scroll size up and its client size down — a pixel
 *  apart with nothing hidden — so a pixel's difference proves nothing;
 *  more than that is text that does not fit.
 *
 *  `text` is the text's own laid-out width, unrounded, and `room` the
 *  width it has, for an element drawn with an ellipsis; null for any other.
 *  Within that pixel they decide: a name overflowing by half a pixel still
 *  gets its ellipsis, and so its tip. */
export function overflows(scroll: number, client: number, text: number | null, room: number): boolean {
  if (scroll - client > 1) return true;
  if (text === null) return false;
  return text > room + 0.5;
}

/** The anchor for a tip over `target`, or null to centre on it.
 *
 *  A truncation tip starts where its text starts (`textLeft`), whatever the
 *  target's width: it is the rest of that text. Any other tip centres on a
 *  target of a control's size, and over a wider one — a row, a heading —
 *  goes by the pointer, or failing one (keyboard focus) by the text's
 *  start, so it is never in the middle of a wide row, far from both. */
export function tipAnchor(
  target: TipTarget,
  overflow: boolean,
  textLeft: number,
  pointerX: number | null,
  wide = WIDE_TARGET,
): TipAnchor | null {
  if (overflow) return { x: textLeft, align: "start" };
  if (target.right - target.left <= wide) return null;
  if (pointerX !== null) return { x: pointerX, align: "centre" };
  return { x: textLeft, align: "start" };
}

/** Lower bound wins: a tip wider than the window starts at the margin and
 *  runs off the far side rather than starting off the near one, so its first
 *  words — the ones that matter — are always readable. */
function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(value, max));
}
