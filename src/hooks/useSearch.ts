// Debounced calls to the backend's single nucleo index. No matching happens
// here: the filter and the palette must agree, and they only agree if there
// is exactly one matcher, in Rust.

import { useEffect, useState } from "react";
import { backend } from "../backend";
import { errorMessage, failed, idle, loading, ready, type Async } from "../logic/async";
import type { ChannelDto } from "../types";

export const SEARCH_DEBOUNCE_MS = 120;

export function useSearch(
  query: string,
  limit: number,
  debounceMs = SEARCH_DEBOUNCE_MS,
): Async<ChannelDto[]> {
  const [result, setResult] = useState<Async<ChannelDto[]>>(idle());

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
        .search(q, limit)
        .then((rows) => {
          // Cancellation is what keeps a slow early keystroke from landing on
          // top of a fast later one.
          if (!cancelled) setResult(ready(rows));
        })
        .catch((e: unknown) => {
          if (!cancelled) setResult(failed(errorMessage(e)));
        });
    }, debounceMs);

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [query, limit, debounceMs]);

  return result;
}
