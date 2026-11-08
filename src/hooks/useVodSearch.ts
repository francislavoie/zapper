// An on-demand tab's search: the service's index, asked after a pause in the
// typing, like the channel search. A later keystroke cancels an earlier one,
// so a slow answer to "lan" never lands over the answer to "lantern". What is
// searched (movies, shows) is the caller's `search`.

import { useEffect, useRef, useState } from "react";
import { errorMessage, failed, idle, loading, ready, type Async } from "../logic/async";
import { SEARCH_DEBOUNCE_MS } from "./useSearch";

/** The most titles one search lists. */
export const VOD_SEARCH_LIMIT = 500;

/** `search` is read when the pause ends, not a reason to search again: the
 *  query alone decides when to ask, so an inline function is fine. */
export function useVodSearch<T>(
  query: string,
  search: (q: string, limit: number) => Promise<T[]>,
): Async<T[]> {
  const [result, setResult] = useState<Async<T[]>>(idle());
  const searchRef = useRef(search);
  searchRef.current = search;

  useEffect(() => {
    const q = query.trim();
    if (q === "") {
      setResult(idle());
      return;
    }
    setResult(loading());
    let cancelled = false;
    const timer = window.setTimeout(() => {
      searchRef
        .current(q, VOD_SEARCH_LIMIT)
        .then((rows) => {
          if (!cancelled) setResult(ready(rows));
        })
        .catch((e: unknown) => {
          if (!cancelled) setResult(failed(errorMessage(e)));
        });
    }, SEARCH_DEBOUNCE_MS);

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [query]);

  return result;
}
