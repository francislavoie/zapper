// Paged access to a list whose length comes with its first page: fetched as
// the window moves, kept while it shows, and asked again when `revision`
// says what it holds may have changed. The bookkeeping, and what keeps a late
// answer out of the wrong list, is in logic/pagedRows.ts.

import { useCallback, useEffect, useReducer, useRef } from "react";
import { errorMessage } from "../logic/async";
import { PageBook } from "../logic/pagedRows";
import { PAGE_SIZE } from "../logic/virtual";

export interface PagedRows<T> {
  total: number;
  rowAt: (index: number) => T | undefined;
  ensure: (start: number, end: number) => void;
  loading: boolean;
  error: string | null;
  retry: () => void;
}

export function usePagedRows<T>(
  key: string,
  fetchPage: (offset: number, limit: number) => Promise<{ total: number; items: T[] }>,
  revision: number,
  /** A cache several lists share, so a second view of the same list (the
   *  Movies list stacked under the picture) neither fetches again nor starts
   *  empty. Without one the list has its own. */
  shared?: PageBook<T>,
): PagedRows<T> {
  const [, bump] = useReducer((n: number) => n + 1, 0);
  const held = useRef<PageBook<T> | null>(null);
  held.current ??= shared ?? new PageBook<T>();
  const book = held.current;
  // The reset happens during render, so another list never shows the last
  // one's rows for a frame, and a refresh does not fetch into a cache that
  // is about to be thrown away.
  book.sync(key, revision);

  // What asks for a page is whatever this render was given, so a request is
  // always about the list on screen: read through a ref rather than closed
  // over, and `ensure` keeps its identity while the list does not change.
  const fetchRef = useRef(fetchPage);
  fetchRef.current = fetchPage;

  /** The window last asked for, so a retry can ask for the same one. */
  const pending = useRef<[number, number]>([0, 0]);

  const ensure = useCallback((start: number, end: number) => {
    pending.current = [start, end];
    const b = book;
    for (const page of b.want(start, end)) {
      const ticket = b.ticket;
      if (!b.begin(ticket, page)) continue;
      fetchRef
        .current(page * PAGE_SIZE, PAGE_SIZE)
        .then((r) => {
          b.accept(ticket, page, r.total, r.items);
        })
        .catch((e: unknown) => {
          b.fail(ticket, page, errorMessage(e));
        });
    }
    // `key` and `revision` are what make this a new function when the list
    // changes, and so what makes the list's window effect ask again for the
    // pages it is showing.
  }, [book, key, revision]);

  // Every answer redraws every list on this book, this one included.
  useEffect(() => book.subscribe(bump), [book]);

  const retry = useCallback(() => {
    book.retry();
    ensure(...pending.current);
    bump();
  }, [book, ensure]);

  const rowAt = useCallback((index: number) => book.rowAt(index), [book]);

  return { total: book.total, rowAt, ensure, loading: book.loading, error: book.error, retry };
}
