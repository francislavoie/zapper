// What a Movies or Shows list searches for: the text in the box once the
// typing has paused, after the channel search's pause (SEARCH_DEBOUNCE_MS),
// so that a word typed asks for one list rather than one per letter. Emptying
// the box is at once: the list there was before comes straight back.

import { useEffect, useState } from "react";
import { SEARCH_DEBOUNCE_MS } from "./useSearch";

export function useSettledSearch(text: string): string {
  const typed = text.trim();
  const [settled, setSettled] = useState(typed);

  useEffect(() => {
    if (typed === "") {
      setSettled("");
      return;
    }
    const timer = window.setTimeout(() => setSettled(typed), SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [typed]);

  return typed === "" ? "" : settled;
}
