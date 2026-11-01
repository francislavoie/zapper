// Debounced, paged programme search. A common word can match thousands of
// airings across a week of guide, so results come a page at a time and
// `more` asks for the next one; the backend's order (on now, upcoming,
// catch-up) makes the first page the useful one.

import { useCallback, useEffect, useRef, useState } from "react";
import { backend } from "../backend";
import { errorMessage, failed, idle, loading, ready, type Async } from "../logic/async";
import type { ProgrammeHitDto } from "../types";
import { SEARCH_DEBOUNCE_MS } from "./useSearch";

export interface ProgrammeSearch {
  /** Every page fetched so far, as one list. */
  result: Async<ProgrammeHitDto[]>;
  /** The last page came back full, so there may be another. */
  hasMore: boolean;
  /** A next page is on its way. */
  loadingMore: boolean;
  more: () => void;
}

export function useProgrammeSearch(
  query: string,
  pageSize: number,
  debounceMs = SEARCH_DEBOUNCE_MS,
): ProgrammeSearch {
  const [result, setResult] = useState<Async<ProgrammeHitDto[]>>(idle());
  const [hasMore, setHasMore] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  /** Which query the pages belong to: a page that lands after the query has
   *  moved on belongs to nobody. */
  const generation = useRef(0);

  useEffect(() => {
    const gen = ++generation.current;
    const q = query.trim();
    setHasMore(false);
    setLoadingMore(false);
    if (q === "") {
      setResult(idle());
      return;
    }
    setResult(loading());
    const timer = window.setTimeout(() => {
      backend
        .searchProgrammes(q, 0, pageSize)
        .then((rows) => {
          if (gen !== generation.current) return;
          setResult(ready(rows));
          setHasMore(rows.length === pageSize);
        })
        .catch((e: unknown) => {
          if (gen === generation.current) setResult(failed(errorMessage(e)));
        });
    }, debounceMs);
    return () => window.clearTimeout(timer);
  }, [query, pageSize, debounceMs]);

  const more = useCallback(() => {
    if (result.status !== "ready" || !hasMore || loadingMore) return;
    const gen = generation.current;
    const have = result.data;
    setLoadingMore(true);
    backend
      .searchProgrammes(query.trim(), have.length, pageSize)
      .then((rows) => {
        if (gen !== generation.current) return;
        setResult(ready([...have, ...rows]));
        setHasMore(rows.length === pageSize);
      })
      .catch((e: unknown) => {
        if (gen === generation.current) setResult(failed(errorMessage(e)));
      })
      .finally(() => {
        if (gen === generation.current) setLoadingMore(false);
      });
  }, [result, hasMore, loadingMore, query, pageSize]);

  return { result, hasMore, loadingMore, more };
}
