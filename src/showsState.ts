// The Shows tab's state, a slice of the store with a reducer of its own, as
// the Movies tab's is (see moviesState.ts): the same rail, lists, sorts and
// menus, plus what a show adds: the season its sheet shows, the episode
// playing, an episode's marks beside a show's, and a revision per show so its
// details are read again once something it holds has changed. The store
// delegates every `shows/*` action here.

import type { Async } from "./logic/async.ts";
import { idle } from "./logic/async.ts";
import type { CategorySort } from "./logic/categoryOrder.ts";
import type { MoviesView, YearRange } from "./logic/movies.ts";
import { SHOW_RECENT, showsOriginChanged } from "./logic/shows.ts";
import type { MoviesMenu } from "./moviesState.ts";
import type {
  EpisodeDto,
  MarksDto,
  MovieSort,
  PlayingEpisodeDto,
  SeriesCategoryDto,
  SeriesDto,
  ShowsSummaryDto,
} from "./types";

export interface ShowsState {
  /** A SHOW_* id or a SeriesCategoryDto.id. */
  selection: string;
  sort: MovieSort;
  years: YearRange | null;
  view: MoviesView;
  /** The search box. Empty browses the selection. */
  query: string;
  categoryFilter: string;
  categorySort: CategorySort;
  summary: Async<ShowsSummaryDto>;
  categories: Async<SeriesCategoryDto[]>;
  /** The show the details pane shows. */
  selected: SeriesDto | null;
  /** The show whose sheet is open. */
  sheetId: number | null;
  /** The season the sheet shows; `null` for the one it opens on
   *  (logic/shows.ts `firstSeason`). */
  sheetSeason: number | null;
  /** The episode the player has open, with its show, for their names while
   *  the player's own state only knows an id. Kept after an episode played
   *  to its end, for what comes next. */
  playing: PlayingEpisodeDto | null;
  /** The latest marks the service reported for each show this session. */
  seriesMarks: Readonly<Record<number, MarksDto>>;
  /** And for each episode: the ids are another numbering than the shows'. */
  episodeMarks: Readonly<Record<number, MarksDto>>;
  /** The sort or years menu, while one is open. */
  menu: MoviesMenu | null;
  /** Bumped when what a list holds may have changed (a mark, a sync, an
   *  episode played), so the lists read again. */
  revision: number;
  /** Per show, bumped when its episodes' marks or places may have changed,
   *  so its details are read again rather than recalled. */
  detailsRevision: Readonly<Record<number, number>>;
}

export function initialShows(view: MoviesView): ShowsState {
  return {
    selection: SHOW_RECENT,
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
    sheetSeason: null,
    playing: null,
    seriesMarks: {},
    episodeMarks: {},
    menu: null,
    revision: 0,
    detailsRevision: {},
  };
}

export type ShowsAction =
  /** A rail item: leaves any search and whatever was open. */
  | { type: "shows/select"; id: string }
  | { type: "shows/sort"; sort: MovieSort }
  | { type: "shows/years"; range: YearRange | null }
  | { type: "shows/view"; view: MoviesView }
  | { type: "shows/query"; text: string }
  | { type: "shows/menu"; menu: MoviesMenu | null }
  | { type: "shows/categoryFilter"; text: string }
  | { type: "shows/categorySort"; sort: CategorySort }
  | { type: "shows/summary"; value: Async<ShowsSummaryDto> }
  | { type: "shows/categories"; value: Async<SeriesCategoryDto[]> }
  | { type: "shows/selectShow"; show: SeriesDto | null }
  /** Opens a show's sheet, on the season it opens on; `null` closes it. */
  | { type: "shows/sheet"; id: number | null }
  | { type: "shows/season"; number: number }
  | { type: "shows/playing"; playing: PlayingEpisodeDto | null }
  | { type: "shows/marked"; marks: MarksDto }
  | { type: "shows/episodeMarked"; seriesId: number; marks: MarksDto }
  /** An episode of the show was played, stopped or marked: its details and
   *  the lists are read again. */
  | { type: "shows/played"; seriesId: number }
  /** A sync may have changed the catalogue under the lists. */
  | { type: "shows/catalogue" };

export function isShowsAction(a: { type: string }): a is ShowsAction {
  return a.type.startsWith("shows/");
}

const patchShow = (s: SeriesDto | null, k: MarksDto): SeriesDto | null =>
  s && s.seriesId === k.id ? { ...s, favourite: k.favourite, watched: k.watched } : s;

const patchEpisode = (e: EpisodeDto, k: MarksDto): EpisodeDto => ({
  ...e,
  watched: k.watched,
  position: k.position,
  duration: k.duration,
});

export function showsReducer(s: ShowsState, a: ShowsAction): ShowsState {
  switch (a.type) {
    case "shows/select":
      return {
        ...s,
        selection: a.id,
        query: "",
        selected: null,
        sheetId: null,
        sheetSeason: null,
        menu: null,
      };
    case "shows/sort":
      return { ...s, sort: a.sort, menu: null };
    case "shows/years":
      return { ...s, years: a.range, menu: null };
    case "shows/menu":
      return { ...s, menu: a.menu };
    case "shows/view":
      return { ...s, view: a.view };
    case "shows/query":
      return { ...s, query: a.text };
    case "shows/categoryFilter":
      return { ...s, categoryFilter: a.text };
    case "shows/categorySort":
      return { ...s, categorySort: a.sort };
    case "shows/summary":
      // As the movies' (moviesState.ts): a summary showing is neither blanked
      // by the fetch that refreshes it nor replaced by one that failed.
      if (a.value.status !== "ready" && s.summary.status === "ready") return s;
      // Another account's shows: nothing kept of the old ones applies.
      if (showsOriginChanged(s.summary, a.value)) {
        return {
          ...s,
          summary: a.value,
          selection: SHOW_RECENT,
          query: "",
          selected: null,
          sheetId: null,
          sheetSeason: null,
          seriesMarks: {},
          episodeMarks: {},
          menu: null,
          revision: s.revision + 1,
        };
      }
      return { ...s, summary: a.value };
    case "shows/categories":
      if (a.value.status === "loading" && s.categories.status === "ready") return s;
      return { ...s, categories: a.value };
    case "shows/selectShow":
      return { ...s, selected: a.show };
    case "shows/sheet":
      return a.id === s.sheetId ? s : { ...s, sheetId: a.id, sheetSeason: null };
    case "shows/season":
      return { ...s, sheetSeason: a.number };
    case "shows/playing":
      return { ...s, playing: a.playing };
    case "shows/marked":
      return {
        ...s,
        seriesMarks: { ...s.seriesMarks, [a.marks.id]: a.marks },
        selected: patchShow(s.selected, a.marks),
        playing:
          s.playing?.series.seriesId === a.marks.id
            ? { ...s.playing, series: patchShow(s.playing.series, a.marks) ?? s.playing.series }
            : s.playing,
        revision: s.revision + 1,
      };
    case "shows/episodeMarked":
      return {
        ...s,
        episodeMarks: { ...s.episodeMarks, [a.marks.id]: a.marks },
        playing:
          s.playing?.episode.episodeId === a.marks.id
            ? { ...s.playing, episode: patchEpisode(s.playing.episode, a.marks) }
            : s.playing,
      };
    case "shows/played":
      return {
        ...s,
        detailsRevision: {
          ...s.detailsRevision,
          [a.seriesId]: (s.detailsRevision[a.seriesId] ?? 0) + 1,
        },
        revision: s.revision + 1,
      };
    case "shows/catalogue":
      // The lists read again, with the marks as the service has them now.
      return { ...s, seriesMarks: {}, episodeMarks: {}, revision: s.revision + 1 };
  }
}
