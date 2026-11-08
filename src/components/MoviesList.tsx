// The Movies tab's main pane: OnDemandList with the movie model — where the
// titles come from, what a row and a card draw, the words, and what a choice
// does. Everything here is memoised so the list's rows keep their renders.

import { useCallback, useMemo } from "react";
import { backend } from "../backend";
import { useMovieSearch } from "../hooks/useMovieSearch";
import {
  MOVIE_SEARCH_PLACEHOLDER,
  movieCard,
  movieCount,
  movieQuery,
  movieRailKind,
  movieTitle,
  type MoviesView,
  type YearRange,
} from "../logic/movies";
import { PageBook } from "../logic/pagedRows";
import type { MoviesMenu } from "../moviesState";
import { saveMoviesView } from "../moviesView";
import { useApp, useDispatch } from "../store";
import type { MovieDto, MovieSort } from "../types";
import { MovieRow } from "./MovieRow";
import { OnDemandList } from "./OnDemandList";

/** The one cache every Movies list draws from: the browse grid's and the one
 *  stacked under the picture show the same titles. */
const MOVIE_PAGES = new PageBook<MovieDto>();

const MOVIE_COLUMNS = ["", "Title", "Year", "Length", "Rating", ""] as const;
const idOf = (m: MovieDto) => m.movieId;
const withMarks = (m: MovieDto, k: { favourite: boolean; watched: boolean }): MovieDto => ({
  ...m,
  favourite: k.favourite,
  watched: k.watched,
});

/** `besideRail`: this list shares the browse grid with the rail, so the
 *  heading carries the button that folds it. Stacked under the picture there
 *  is no rail to fold. */
export function MoviesList({ besideRail = true }: { besideRail?: boolean }) {
  const { movies } = useApp();
  const dispatch = useDispatch();

  const query = useMemo(
    () => movieQuery(movies.selection, movies.sort, movies.years),
    [movies.selection, movies.sort, movies.years],
  );
  const listKey = JSON.stringify(query);
  const fetchPage = useCallback(
    (offset: number, limit: number) => backend.movies(query, offset, limit),
    [query],
  );
  const search = useMovieSearch(movies.query);

  const searching = movies.query.trim() !== "";
  const categories = movies.categories.status === "ready" ? movies.categories.data : [];
  const heading = searching ? "Search results" : movieTitle(movies.selection, categories);

  const selectedId = movies.selected?.movieId ?? null;
  const state = useMemo(
    () => ({
      query: movies.query,
      view: movies.view,
      sort: movies.sort,
      years: movies.years,
      menu: movies.menu,
      revision: movies.revision,
      selectedId,
    }),
    [movies.query, movies.view, movies.sort, movies.years, movies.menu, movies.revision, selectedId],
  );

  const selected = movies.selected;
  const on = useMemo(
    () => ({
      select: (m: MovieDto) => dispatch({ type: "movies/selectMovie", movie: m }),
      open: (m: MovieDto) => {
        dispatch({ type: "movies/selectMovie", movie: m });
        dispatch({ type: "movies/sheet", id: m.movieId });
      },
      // Enter opens the selected title, wherever it was chosen: the sheet's
      // shelf picks titles this list may not hold.
      openSelected: () => {
        if (selected) dispatch({ type: "movies/sheet", id: selected.movieId });
      },
      query: (text: string) => dispatch({ type: "movies/query", text }),
      menu: (menu: MoviesMenu | null) => dispatch({ type: "movies/menu", menu }),
      view: (view: MoviesView) => {
        saveMoviesView(view);
        dispatch({ type: "movies/view", view });
      },
      sort: (sort: MovieSort) => dispatch({ type: "movies/sort", sort }),
      years: (range: YearRange | null) => dispatch({ type: "movies/years", range }),
    }),
    [dispatch, selected],
  );

  return (
    <OnDemandList<MovieDto>
      besideRail={besideRail}
      idPrefix="movie"
      kindClass="movies"
      colsClass="movie-cols"
      unavailableLabel="Movies unavailable"
      listKey={listKey}
      fetchPage={fetchPage}
      pages={MOVIE_PAGES}
      search={search}
      state={state}
      marks={movies.marks}
      idOf={idOf}
      withMarks={withMarks}
      heading={heading}
      countLabel={movieCount}
      placeholder={MOVIE_SEARCH_PLACEHOLDER}
      emptyNote={emptyNote(movies.selection, searching, movies.query, movies.years)}
      columns={MOVIE_COLUMNS}
      Row={MovieRow}
      card={movieCard}
      on={on}
    />
  );
}

/** What an empty list says: nothing here is an error, only an empty list. */
function emptyNote(
  selection: string,
  searching: boolean,
  query: string,
  years: YearRange | null,
): string {
  if (searching) return `Nothing matches “${query.trim()}”.`;
  if (years) return "No movies from those years.";
  switch (movieRailKind(selection)) {
    case "favourites":
      return "Nothing here yet. Movies you mark as favourites are listed here.";
    case "watched":
      return "Nothing here yet. Movies you finish, or mark as watched, are listed here.";
    case "recent":
      return "Nothing has been added lately.";
    default:
      return "No movies here.";
  }
}
