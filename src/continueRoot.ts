// What Continue watching does across the store's slices: another account
// resets it, its first list opens each tab on it, a sync moves it. Kept out of
// store.tsx, as plain `.ts`, so a node check can run it on the three slices.

import { continueReducer, isContinueAction, type ContinueAction, type ContinueState } from "./continueState.ts";
import { CONTINUE, firstSelection } from "./logic/continueWatching.ts";
import { MOVIE_RECENT, moviesOriginChanged } from "./logic/movies.ts";
import { SHOW_RECENT, showsOriginChanged } from "./logic/shows.ts";
import { isMoviesAction, moviesReducer, type MoviesAction, type MoviesState } from "./moviesState.ts";
import { isShowsAction, showsReducer, type ShowsAction, type ShowsState } from "./showsState.ts";

export interface Slices {
  movies: MoviesState;
  shows: ShowsState;
  continue: ContinueState;
}

/** Whether an action is one of the three slices'. */
export const isSliceAction = (a: { type: string }): a is MoviesAction | ShowsAction | ContinueAction =>
  isMoviesAction(a) || isShowsAction(a) || isContinueAction(a);

/** A tab opens on Continue watching only while it is still on its own first
 *  selection with nothing the owner opened or typed. */
function openOnContinue<T extends { chosen: boolean; selection: string; selected: unknown; query: string }>(
  t: T,
  recent: string,
  n: number,
): T {
  return t.selected === null && t.query === "" && firstSelection(t.chosen, t.selection, recent, n) === CONTINUE
    ? { ...t, selection: CONTINUE }
    : t;
}

export function reduceSlices(s: Slices, action: MoviesAction | ShowsAction | ContinueAction): Slices {
  const reset = () => continueReducer(s.continue, { type: "continue/reset" });
  if (isMoviesAction(action)) {
    const movies = moviesReducer(s.movies, action);
    // Another account's movies: its Continue watching is not this one's.
    return action.type === "movies/summary" && moviesOriginChanged(s.movies.summary, action.value)
      ? { ...s, movies, continue: reset() }
      : { ...s, movies };
  }
  if (isShowsAction(action)) {
    const shows = showsReducer(s.shows, action);
    return action.type === "shows/summary" && showsOriginChanged(s.shows.summary, action.value)
      ? { ...s, shows, continue: reset() }
      : { ...s, shows };
  }
  const next = continueReducer(s.continue, action);
  if (next === s.continue) return s;
  if (action.type !== "continue/items" || action.value.status !== "ready") {
    return { ...s, continue: next };
  }
  const n = action.value.data.length;
  return {
    ...s,
    continue: next,
    movies: openOnContinue(s.movies, MOVIE_RECENT, n),
    shows: openOnContinue(s.shows, SHOW_RECENT, n),
  };
}

/** A sync that changed the catalogue: the lists and Continue watching read
 *  again. */
export function syncedSlices(s: Slices): Slices {
  return {
    movies: moviesReducer(s.movies, { type: "movies/catalogue" }),
    shows: showsReducer(s.shows, { type: "shows/catalogue" }),
    continue: continueReducer(s.continue, { type: "continue/changed" }),
  };
}
