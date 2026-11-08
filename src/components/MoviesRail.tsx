// The Movies tab's rail: the lists that are always there (Continue watching,
// Favourites, Recently added, Watched), then the provider's movie categories. The rail
// itself is OnDemandRail; this says what is on it and what a choice does.

import { useCallback, useMemo } from "react";
import { loadMovieCategories } from "../actions";
import type { Async } from "../logic/async";
import type { CategorySort } from "../logic/categoryOrder";
import { CONTINUE } from "../logic/continueWatching";
import { MOVIE_FAVOURITES, MOVIE_WATCHED, fixedMovieItems } from "../logic/movies";
import { useApp, useDispatch } from "../store";
import { OnDemandRail } from "./OnDemandRail";

export function MoviesRail() {
  const { movies, continue: cw } = useApp();
  const { summary, categories, selection, categoryFilter, categorySort } = movies;
  const dispatch = useDispatch();

  // Continue watching's count is blank until its list has been read.
  const continueCount = cw.items.status === "ready" ? cw.items.data.length : 0;
  const fixed = useMemo(
    () => fixedMovieItems(summary.status === "ready" ? summary.data : null, continueCount),
    [summary, continueCount],
  );
  const counted = useMemo(
    (): Async<{ id: string; name: string; count: number }[]> =>
      categories.status === "ready"
        ? { ...categories, data: categories.data.map((c) => ({ id: c.id, name: c.name, count: c.movieCount })) }
        : categories,
    [categories],
  );

  const onSelect = useCallback((id: string) => dispatch({ type: "movies/select", id }), [dispatch]);
  const onFilter = useCallback((text: string) => dispatch({ type: "movies/categoryFilter", text }), [dispatch]);
  const onSort = useCallback((sort: CategorySort) => dispatch({ type: "movies/categorySort", sort }), [dispatch]);
  const onRetry = useCallback(() => void loadMovieCategories(dispatch), [dispatch]);

  return (
    <OnDemandRail
      fixed={fixed}
      categories={counted}
      selection={selection}
      filter={categoryFilter}
      sort={categorySort}
      icon={movieRailIcon}
      onSelect={onSelect}
      onFilter={onFilter}
      onSort={onSort}
      onRetry={onRetry}
    />
  );
}

/** The mark before a fixed row's name: ▶ before Continue watching, a star
 *  before Favourites, a tick before Watched. */
function movieRailIcon(id: string) {
  if (id === CONTINUE) return <span className="rail-fixed-i" aria-hidden>▶</span>;
  if (id === MOVIE_FAVOURITES) return <span className="rail-fixed-i" aria-hidden>★</span>;
  if (id === MOVIE_WATCHED) return <span className="rail-fixed-i" aria-hidden>✓</span>;
  return null;
}
