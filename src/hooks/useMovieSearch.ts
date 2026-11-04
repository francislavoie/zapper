// The Movies tab's search: the service's index, asked after a pause in the
// typing, like the channel search. A later keystroke cancels an earlier one,
// so a slow answer to "lan" never lands over the answer to "lantern".

import { useEffect, useState } from "react";
import { backend } from "../backend";
import { errorMessage, failed, idle, loading, ready, type Async } from "../logic/async";
import type { MovieDto } from "../types";
import { SEARCH_DEBOUNCE_MS } from "./useSearch";

/** The most titles one search lists. */
export const MOVIE_SEARCH_LIMIT = 500;

export function useMovieSearch(query: string): Async<MovieDto[]> {
  const [result, setResult] = useState<Async<MovieDto[]>>(idle());

  useEffect(() => {
    const q = query.trim();
    if (q === "") {
      setResult(idle());
      return;
    }
    setResult(loading());
    let cancelled = false;
    const timer = window.setTimeout(() => {
      backend
        .searchMovies(q, MOVIE_SEARCH_LIMIT)
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
