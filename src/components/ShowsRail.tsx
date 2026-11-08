// The Shows tab's rail: the lists that are always there (Continue watching,
// Favourites, Recently added, Watched), then the provider's show categories. The rail
// itself is OnDemandRail; this says what is on it and what a choice does.

import { useCallback, useMemo } from "react";
import { loadShowCategories } from "../actions";
import type { Async } from "../logic/async";
import type { CategorySort } from "../logic/categoryOrder";
import { CONTINUE } from "../logic/continueWatching";
import { SHOW_FAVOURITES, SHOW_WATCHED, fixedShowItems } from "../logic/shows";
import { useApp, useDispatch } from "../store";
import { OnDemandRail } from "./OnDemandRail";

export function ShowsRail() {
  const { shows, continue: cw } = useApp();
  const { summary, categories, selection, categoryFilter, categorySort } = shows;
  const dispatch = useDispatch();

  // Continue watching's count is blank until its list has been read.
  const continueCount = cw.items.status === "ready" ? cw.items.data.length : 0;
  const fixed = useMemo(
    () => fixedShowItems(summary.status === "ready" ? summary.data : null, continueCount),
    [summary, continueCount],
  );
  const counted = useMemo(
    (): Async<{ id: string; name: string; count: number }[]> =>
      categories.status === "ready"
        ? { ...categories, data: categories.data.map((c) => ({ id: c.id, name: c.name, count: c.seriesCount })) }
        : categories,
    [categories],
  );

  const onSelect = useCallback((id: string) => dispatch({ type: "shows/select", id }), [dispatch]);
  const onFilter = useCallback((text: string) => dispatch({ type: "shows/categoryFilter", text }), [dispatch]);
  const onSort = useCallback((sort: CategorySort) => dispatch({ type: "shows/categorySort", sort }), [dispatch]);
  const onRetry = useCallback(() => void loadShowCategories(dispatch), [dispatch]);

  return (
    <OnDemandRail
      fixed={fixed}
      categories={counted}
      selection={selection}
      filter={categoryFilter}
      sort={categorySort}
      icon={showRailIcon}
      onSelect={onSelect}
      onFilter={onFilter}
      onSort={onSort}
      onRetry={onRetry}
    />
  );
}

/** The mark before a fixed row's name: ▶ before Continue watching, a star
 *  before Favourites, a tick before Watched. */
function showRailIcon(id: string) {
  if (id === CONTINUE) return <span className="rail-fixed-i" aria-hidden>▶</span>;
  if (id === SHOW_FAVOURITES) return <span className="rail-fixed-i" aria-hidden>★</span>;
  if (id === SHOW_WATCHED) return <span className="rail-fixed-i" aria-hidden>✓</span>;
  return null;
}
