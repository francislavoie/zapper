// Windowing arithmetic for the channel list. Tens of thousands of rows cannot
// all exist as DOM nodes, so the list renders only what fits plus a little overscan and
// positions each row absolutely inside a spacer of the full height.
//
// All of this is pure and checked in src/__checks__/virtual.test.mjs.

/** Dense rows: ~40 fit a normal window, against the 8 the grid managed. */
export const ROW_HEIGHT = 26;
/** The channel list's two-line rows (name over category, on now with its
 *  progress); the rail keeps the dense ROW_HEIGHT. */
export const LIST_ROW_HEIGHT = 40;
export const OVERSCAN = 8;
export const PAGE_SIZE = 200;

export interface Windowing {
  /** First index to render, inclusive. */
  start: number;
  /** One past the last index to render. */
  end: number;
  /** Pixel offset of `start` inside the spacer. */
  offsetY: number;
  /** Height of the spacer that gives the scrollbar its range. */
  totalHeight: number;
}

export function computeWindow(
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  total: number,
  overscan: number,
): Windowing {
  const h = rowHeight > 0 ? rowHeight : 1;
  const totalHeight = Math.max(0, total) * h;
  if (total <= 0) return { start: 0, end: 0, offsetY: 0, totalHeight: 0 };

  const top = Math.max(0, Math.min(scrollTop, Math.max(0, totalHeight - 1)));
  const firstVisible = Math.floor(top / h);
  const lastVisible = Math.floor((top + Math.max(0, viewportHeight) - 1) / h);

  const start = Math.max(0, firstVisible - overscan);
  const end = Math.min(total, lastVisible + 1 + overscan);
  return { start, end: Math.max(start, end), offsetY: start * h, totalHeight };
}

export const pageOf = (index: number, pageSize: number): number =>
  Math.floor(index / pageSize);

/** The pages a window touches, inclusive of both ends. */
export function pagesForWindow(
  start: number,
  end: number,
  pageSize: number,
): number[] {
  if (end <= start) return [];
  const first = pageOf(start, pageSize);
  const last = pageOf(end - 1, pageSize);
  const out: number[] = [];
  for (let p = first; p <= last; p++) out.push(p);
  return out;
}

/** Pages a window needs that are neither loaded nor already in flight. */
export function missingPages(
  start: number,
  end: number,
  pageSize: number,
  known: (page: number) => boolean,
): number[] {
  return pagesForWindow(start, end, pageSize).filter((p) => !known(p));
}

/** How far the list can be scrolled before more data is required.
 *
 *  `channelCount` excludes separators, so a category's real row count is
 *  always at least its count and usually a bit more. Rather than guess, the
 *  list treats the total as a lower bound that grows as pages come back, and
 *  becomes exact the moment a short page proves the end. */
export function projectedTotal(
  estimate: number,
  loadedThrough: number,
  pageSize: number,
  exact: number | null,
): number {
  if (exact !== null) return Math.max(0, exact);
  return Math.max(estimate, loadedThrough + (loadedThrough > 0 ? pageSize : 0));
}

/** The header pinned at the top of the viewport: the nearest separator at or
 *  above the first visible row. Scanning is bounded because separators run
 *  about every sixty rows and an unbounded walk over tens of thousands
 *  would be silly. */
export function stickyIndex(
  isSeparator: (index: number) => boolean | undefined,
  firstVisible: number,
  lookback = 400,
): number | null {
  for (let i = firstVisible; i >= 0 && firstVisible - i <= lookback; i--) {
    const sep = isSeparator(i);
    if (sep === undefined) return null; // unloaded: say nothing rather than lie
    if (sep) return i;
  }
  return null;
}
