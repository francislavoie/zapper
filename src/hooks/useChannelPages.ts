// Paged access to a category's rows, fetched on demand as the window moves.
//
// The list never knows how many rows a category really has until it hits the
// end: `channelCount` excludes separators, so it is a floor, not a total. The
// projection in logic/virtual.ts handles that; this hook just keeps pages.
//
// Pages are kept for as long as the category is showing, so a sync that
// rewrites the catalogue has to say so: `revision` changes when it has (see
// logic/catalogueRefresh.ts), and every page is then asked for again. The
// old pages stay on screen until their replacements arrive, so a refresh
// does not blank the list or move its scroll position.

import { useCallback, useReducer, useRef } from "react";
import { backend } from "../backend";
import { errorMessage } from "../logic/async";
import { refreshingTotal } from "../logic/catalogueRefresh";
import { PAGE_SIZE, missingPages, pageOf, projectedTotal } from "../logic/virtual";
import type { ChannelDto } from "../types";

export interface RowSource {
  total: number;
  rowAt: (index: number) => ChannelDto | undefined;
  ensure: (start: number, end: number) => void;
  error: string | null;
  loading: boolean;
  retry: () => void;
}

export function useChannelPages(
  categoryId: string | null,
  estimate: number,
  revision = 0,
): RowSource {
  const [, bump] = useReducer((n: number) => n + 1, 0);
  const pages = useRef(new Map<number, ChannelDto[]>());
  /** What was showing before a refresh, drawn until each page is replaced.
   *  Empty when nothing is being refreshed. */
  const stale = useRef(new Map<number, ChannelDto[]>());
  /** The length the list claimed before a refresh; `null` when none is under
   *  way, or once the category changes. */
  const staleTotal = useRef<number | null>(null);
  const inflight = useRef(new Set<number>());
  const failedPages = useRef(new Set<number>());
  const exact = useRef<number | null>(null);
  const loadedThrough = useRef(0);
  const error = useRef<string | null>(null);
  /** Bumped on every reset, so an answer to a question asked before it is
   *  dropped rather than written over the newer state. */
  const generation = useRef(0);

  // What the list claims as this render starts, before any reset below: a
  // refresh keeps it as a floor.
  const total = refreshingTotal(
    projectedTotal(estimate, loadedThrough.current, PAGE_SIZE, exact.current),
    exact.current,
    staleTotal.current,
  );

  // Both resets happen during render rather than in an effect: switching
  // category must not show the previous one's rows for a frame, and a
  // refresh must not fetch into a cache it is about to throw away.
  const key = categoryId ?? "*all*";
  const keyRef = useRef(key);
  const revisionRef = useRef(revision);
  if (keyRef.current !== key || revisionRef.current !== revision) {
    const sameList = keyRef.current === key;
    if (sameList) {
      // Pages fetched before the refresh outrank anything older still kept.
      for (const [p, rows] of pages.current) stale.current.set(p, rows);
      staleTotal.current = Math.max(total, staleTotal.current ?? 0);
    } else {
      stale.current = new Map();
      staleTotal.current = null;
    }
    keyRef.current = key;
    revisionRef.current = revision;
    pages.current = new Map();
    inflight.current = new Set();
    failedPages.current = new Set();
    exact.current = null;
    loadedThrough.current = 0;
    error.current = null;
    generation.current += 1;
  }

  const fetchPage = useCallback(
    (page: number) => {
      const mine = generation.current;
      inflight.current.add(page);
      backend
        .channels(categoryId, page * PAGE_SIZE, PAGE_SIZE)
        .then((rows) => {
          if (generation.current !== mine) return;
          inflight.current.delete(page);
          pages.current.set(page, rows);
          stale.current.delete(page);
          loadedThrough.current = Math.max(
            loadedThrough.current,
            page * PAGE_SIZE + rows.length,
          );
          // A short page is the only honest proof of where the list ends. An
          // empty page past the end proves less than a short one before it,
          // so the nearer end wins whichever answers first.
          if (rows.length < PAGE_SIZE) {
            const end = page * PAGE_SIZE + rows.length;
            exact.current = exact.current === null ? end : Math.min(exact.current, end);
          }
          if (exact.current !== null) {
            for (const p of [...stale.current.keys()]) {
              if (p * PAGE_SIZE >= exact.current) stale.current.delete(p);
            }
          }
          error.current = null;
          bump();
        })
        .catch((e: unknown) => {
          if (generation.current !== mine) return;
          inflight.current.delete(page);
          // Remember the failure so a scroll does not hammer a dead backend.
          failedPages.current.add(page);
          error.current = errorMessage(e);
          bump();
        });
    },
    // `revision` is not read here, but it is what makes `ensure` a new
    // function after a refresh, and so what makes the list's window effect
    // ask again for the pages it is showing.
    [categoryId, revision],
  );

  const ensure = useCallback(
    (start: number, end: number) => {
      const known = (p: number) =>
        pages.current.has(p) || inflight.current.has(p) || failedPages.current.has(p);
      const want = missingPages(start, end, PAGE_SIZE, known);
      // While refreshing, the first page as well: it is the cheapest way to
      // learn that the list is now shorter than the screen being shown, when
      // the reader is scrolled far down a catalogue that has since shrunk.
      if (staleTotal.current !== null && !known(0) && !want.includes(0)) want.push(0);
      for (const p of want) fetchPage(p);
    },
    [fetchPage],
  );

  const retry = useCallback(() => {
    failedPages.current = new Set();
    error.current = null;
    bump();
  }, []);

  const rowAt = useCallback((index: number): ChannelDto | undefined => {
    const p = pageOf(index, PAGE_SIZE);
    const page = pages.current.get(p) ?? stale.current.get(p);
    return page?.[index % PAGE_SIZE];
  }, []);

  return {
    total: refreshingTotal(
      projectedTotal(estimate, loadedThrough.current, PAGE_SIZE, exact.current),
      exact.current,
      staleTotal.current,
    ),
    rowAt,
    ensure,
    error: error.current,
    loading: inflight.current.size > 0,
    retry,
  };
}
