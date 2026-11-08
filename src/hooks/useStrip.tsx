// What a tab hands its list as `above`: the Continue watching strip while it
// is shown (logic/continueWatching.ts `stripShown`).

import { useCallback, type ReactNode } from "react";
import { ContinueStrip } from "../components/ContinueStrip";
import { stripShown } from "../logic/continueWatching";
import type { MoviesView } from "../logic/movies";
import { useApp } from "../store";

export function useStrip(
  tab: "movies" | "shows",
  view: MoviesView,
  selection: string,
  query: string,
): (paneHeight: number) => ReactNode {
  const items = useApp().continue.items;
  const count = items.status === "ready" ? items.data.length : 0;
  return useCallback(
    (paneHeight: number) =>
      stripShown(view, selection, query, count, paneHeight) ? <ContinueStrip tab={tab} /> : null,
    [view, selection, query, count, tab],
  );
}

