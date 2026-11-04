// The Movies tab's state, a slice of the store with a reducer of its own. The
// panes, the watch band and the keys all read it, so it lives in the store;
// but it shares nothing with Live TV's state, and a reducer of its own keeps
// the store's from growing a second tab's worth of cases, and stays plain
// `.ts` so a node check can run it. The store delegates every `movies/*`
// action here.

import type { Async } from "./logic/async.ts";
import { idle } from "./logic/async.ts";
import type { CategorySort } from "./logic/categoryOrder.ts";
import {
  MOVIE_RECENT,
  moviesOriginChanged,
  type MoviesView,
  type YearRange,
} from "./logic/movies.ts";
import type {
  MarksDto,
  MovieDto,
  MovieSort,
  MoviesSummaryDto,
  VodCategoryDto,
} from "./types";

/** The list header's open menu and where it is drawn, in window pixels. */
export interface MoviesMenu {
  kind: "sort" | "years";
  x: number;
  y: number;
}

export interface MoviesState {
  /** A MOVIE_* id or a VodCategoryDto.id. */
  selection: string;
  sort: MovieSort;
  years: YearRange | null;
  view: MoviesView;
  /** The search box. Empty browses the selection. */
  query: string;
  categoryFilter: string;
  categorySort: CategorySort;
  summary: Async<MoviesSummaryDto>;
  categories: Async<VodCategoryDto[]>;
  /** The title the details pane shows. */
  selected: MovieDto | null;
  /** The title whose bottom sheet is open, in a narrow window. */
  sheetId: number | null;
  /** The title the player has open, for its name while the player's own
   *  state only knows an id. */
  playing: MovieDto | null;
  /** The latest marks the service reported for each title this session. */
  marks: Readonly<Record<number, MarksDto>>;
  /** The sort or years menu, while one is open. */
  menu: MoviesMenu | null;
  /** Bumped when what a list holds may have changed (a mark, a sync), so
   *  the lists read again. */
  revision: number;
}

export function initialMovies(view: MoviesView): MoviesState {
  return {
    selection: MOVIE_RECENT,
    sort: "added",
    years: null,
    view,
    query: "",
    categoryFilter: "",
    categorySort: "provider",
    summary: idle(),
    categories: idle(),
    selected: null,
    sheetId: null,
    playing: null,
    marks: {},
    menu: null,
    revision: 0,
  };
}

export type MoviesAction =
  /** A rail item: leaves any search and whatever was open. */
  | { type: "movies/select"; id: string }
  | { type: "movies/sort"; sort: MovieSort }
  | { type: "movies/years"; range: YearRange | null }
  | { type: "movies/view"; view: MoviesView }
  | { type: "movies/query"; text: string }
  | { type: "movies/menu"; menu: MoviesMenu | null }
  | { type: "movies/categoryFilter"; text: string }
  | { type: "movies/categorySort"; sort: CategorySort }
  | { type: "movies/summary"; value: Async<MoviesSummaryDto> }
  | { type: "movies/categories"; value: Async<VodCategoryDto[]> }
  | { type: "movies/selectMovie"; movie: MovieDto | null }
  | { type: "movies/sheet"; id: number | null }
  | { type: "movies/playing"; movie: MovieDto | null }
  | { type: "movies/marked"; marks: MarksDto }
  /** A sync may have changed the catalogue under the lists. */
  | { type: "movies/catalogue" };

export function isMoviesAction(a: { type: string }): a is MoviesAction {
  return a.type.startsWith("movies/");
}

const patch = (m: MovieDto | null, k: MarksDto): MovieDto | null =>
  m && m.movieId === k.id ? { ...m, favourite: k.favourite, watched: k.watched } : m;

export function moviesReducer(s: MoviesState, a: MoviesAction): MoviesState {
  switch (a.type) {
    case "movies/select":
      return { ...s, selection: a.id, query: "", selected: null, sheetId: null, menu: null };
    case "movies/sort":
      return { ...s, sort: a.sort, menu: null };
    case "movies/years":
      return { ...s, years: a.range, menu: null };
    case "movies/menu":
      return { ...s, menu: a.menu };
    case "movies/view":
      return { ...s, view: a.view };
    case "movies/query":
      return { ...s, query: a.text };
    case "movies/categoryFilter":
      return { ...s, categoryFilter: a.text };
    case "movies/categorySort":
      return { ...s, categorySort: a.sort };
    case "movies/summary":
      // A summary already showing is not blanked by the fetch that refreshes it.
      // Nor is it replaced by a refresh that failed: the tab comes and goes
      // with `available`, and a dropped connection is not the account
      // losing its movies.
      if (a.value.status !== "ready" && s.summary.status === "ready") return s;
      // Another account's movies: nothing kept of the old ones applies,
      // though the ids may be the same.
      if (moviesOriginChanged(s.summary, a.value)) {
        return {
          ...s,
          summary: a.value,
          selection: MOVIE_RECENT,
          query: "",
          selected: null,
          sheetId: null,
          marks: {},
          menu: null,
          revision: s.revision + 1,
        };
      }
      return { ...s, summary: a.value };
    case "movies/categories":
      if (a.value.status === "loading" && s.categories.status === "ready") return s;
      return { ...s, categories: a.value };
    case "movies/selectMovie":
      return { ...s, selected: a.movie };
    case "movies/sheet":
      return { ...s, sheetId: a.id };
    case "movies/playing":
      return { ...s, playing: a.movie };
    case "movies/marked":
      return {
        ...s,
        marks: { ...s.marks, [a.marks.id]: a.marks },
        selected: patch(s.selected, a.marks),
        playing: patch(s.playing, a.marks),
        revision: s.revision + 1,
      };
    case "movies/catalogue":
      // The lists read again, with the marks as the service has them now.
      return { ...s, marks: {}, revision: s.revision + 1 };
  }
}
