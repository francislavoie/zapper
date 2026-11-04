// What the arrow keys do in the Movies list, as arithmetic: where the
// selection goes for a key, in the list and in the posters. Pure; the
// component only has to scroll to the answer. Checked in
// src/__checks__/movieKeys.test.mjs.

export interface CursorContext {
  /** The selected row, or -1 when there is none yet. */
  current: number;
  total: number;
  /** Titles across a row of posters; 1 in the list. */
  columns: number;
  /** Rows that fit on screen, for PageUp and PageDown. */
  visibleRows: number;
  posters: boolean;
}

/** Where the selection goes for `key`, or null when the key is not a
 *  movement here. Never outside the list; nothing at all for an empty one. */
export function movieCursorKey(key: string, c: CursorContext): number | null {
  if (c.total <= 0) return null;
  const page = Math.max(1, c.visibleRows - 1) * c.columns;
  let to: number;
  switch (key) {
    case "ArrowDown":
      to = c.current + c.columns;
      break;
    case "ArrowUp":
      to = c.current - c.columns;
      break;
    // Sideways is the posters' alone: in the list it stays the player's
    // seek, as it is everywhere else.
    case "ArrowRight":
      if (!c.posters) return null;
      to = c.current + 1;
      break;
    case "ArrowLeft":
      if (!c.posters) return null;
      to = c.current - 1;
      break;
    case "PageDown":
      to = c.current + page;
      break;
    case "PageUp":
      to = c.current - page;
      break;
    case "Home":
      to = 0;
      break;
    case "End":
      to = c.total - 1;
      break;
    default:
      return null;
  }
  // From nothing selected, a step forward lands on the first title rather
  // than skipping it.
  if (c.current < 0 && to < 0) to = 0;
  return Math.min(c.total - 1, Math.max(0, to));
}

/** The scroll offset that brings the row holding `index` into view, moving
 *  as little as it can; `scrollTop` itself when it is in view already.
 *  `rowTop` is where that row starts and `rowHeight` how tall it is. */
export function scrollToShow(
  rowTop: number,
  rowHeight: number,
  scrollTop: number,
  viewport: number,
): number {
  if (rowTop < scrollTop) return rowTop;
  if (rowTop + rowHeight > scrollTop + viewport) return Math.max(0, rowTop + rowHeight - viewport);
  return scrollTop;
}
