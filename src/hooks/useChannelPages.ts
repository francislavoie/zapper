// Paged access to a category's rows, fetched on demand as the window moves.
//
// The list never knows how many rows a category really has until it hits the
// end: `channelCount` excludes separators, so it is a floor, not a total. The
// projection in logic/virtual.ts handles that; this hook just keeps pages.

import { useCallback, useReducer, useRef } from "react";
import { backend } from "../backend";
import { errorMessage } from "../logic/async";
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

export function useChannelPages(categoryId: string | null, estimate: number): RowSource {
  const [, bump] = useReducer((n: number) => n + 1, 0);
  const pages = useRef(new Map<number, ChannelDto[]>());
  const inflight = useRef(new Set<number>());
  const failedPages = useRef(new Set<number>());
  const exact = useRef<number | null>(null);
  const loadedThrough = useRef(0);
  const error = useRef<string | null>(null);

  // Switching category must not show the previous one's rows for a frame, so
  // the reset happens during render rather than in an effect.
  const key = categoryId ?? "*all*";
  const keyRef = useRef(key);
  if (keyRef.current !== key) {
    keyRef.current = key;
    pages.current = new Map();
    inflight.current = new Set();
    failedPages.current = new Set();
    exact.current = null;
    loadedThrough.current = 0;
    error.current = null;
  }

  const fetchPage = useCallback(
    (page: number) => {
      const mine = keyRef.current;
      inflight.current.add(page);
      backend
        .channels(categoryId, page * PAGE_SIZE, PAGE_SIZE)
        .then((rows) => {
          if (keyRef.current !== mine) return;
          inflight.current.delete(page);
          pages.current.set(page, rows);
          loadedThrough.current = Math.max(
            loadedThrough.current,
            page * PAGE_SIZE + rows.length,
          );
          // A short page is the only honest proof of where the list ends.
          if (rows.length < PAGE_SIZE) {
            exact.current = page * PAGE_SIZE + rows.length;
          }
          error.current = null;
          bump();
        })
        .catch((e: unknown) => {
          if (keyRef.current !== mine) return;
          inflight.current.delete(page);
          // Remember the failure so a scroll does not hammer a dead backend.
          failedPages.current.add(page);
          error.current = errorMessage(e);
          bump();
        });
    },
    [categoryId],
  );

  const ensure = useCallback(
    (start: number, end: number) => {
      const want = missingPages(
        start,
        end,
        PAGE_SIZE,
        (p) => pages.current.has(p) || inflight.current.has(p) || failedPages.current.has(p),
      );
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
    const page = pages.current.get(pageOf(index, PAGE_SIZE));
    return page?.[index % PAGE_SIZE];
  }, []);

  return {
    total: projectedTotal(estimate, loadedThrough.current, PAGE_SIZE, exact.current),
    rowAt,
    ensure,
    error: error.current,
    loading: inflight.current.size > 0,
    retry,
  };
}
