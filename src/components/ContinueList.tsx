// Continue watching as an on-demand tab's main pane: OnDemandList over the
// store's list (continueState.ts), the same on the Movies and the Shows tab.
// Choosing an entry selects it and plays nothing; Enter, a double-click and
// the menu's Resume / Start over / Play are what play.
//
// The tab's own list is its child, drawn in its place when Continue watching
// is not shown: one OnDemandList in one place either way, so typing in the
// search box, which turns Continue watching into the tab's search results,
// leaves the box (the same element) with its focus.

import { useCallback, useMemo, useRef, type ReactElement } from "react";
import { playContinue } from "../actions";
import { CONTINUE_EMPTY, continueCard, continueCount, continueKey } from "../logic/continueWatching";
import { MOVIE_SEARCH_PLACEHOLDER, playingKind, type MoviesView } from "../logic/movies";
import { PageBook } from "../logic/pagedRows";
import { SHOW_SEARCH_PLACEHOLDER, SHOWS_VIEW_STORAGE_KEY } from "../logic/shows";
import { saveMoviesView, saveVodView } from "../moviesView";
import { useApp, useDispatch } from "../store";
import type { ContinueItemDto } from "../types";
import { ContinueRow } from "./ContinueRow";
import { OnDemandList, type MenuAt } from "./OnDemandList";

/** One cache for every Continue watching list: they all show the store's. */
const CONTINUE_PAGES = new PageBook<ContinueItemDto>();

const COLUMNS = ["", "Title", "Year", "Length", "Rating", "Progress", "Last watched"] as const;
const NO_ITEMS: readonly ContinueItemDto[] = [];
const NO_MARKS = {};
const NO_MENU = () => {};
/** The entries carry their own marks. */
const withMarks = (i: ContinueItemDto) => i;
/** Never answers: the list is still being read (the head says loading). */
const pending = () => new Promise<never>(() => {});

/** A number for each list read: the same entries read again (a place moved
 *  on) are another revision of the list, fetched again. One count for every
 *  list showing it, so they agree. */
const versions = new WeakMap<readonly ContinueItemDto[], number>();
let lastVersion = 0;
function versionOf(items: readonly ContinueItemDto[]): number {
  let v = versions.get(items);
  if (v === undefined) {
    v = ++lastVersion;
    versions.set(items, v);
  }
  return v;
}

export interface ContinueListProps {
  tab: "movies" | "shows";
  besideRail: boolean;
  /** Whether Continue watching is what the tab shows: chosen in the rail,
   *  and no search typed there. */
  shown: boolean;
  /** The tab's own list (an OnDemandList), drawn when it is not. */
  children: ReactElement;
}

/** Continue watching on `tab`, or the tab's own list in its place. */
export function ContinueList({ tab, besideRail, shown, children }: ContinueListProps) {
  const app = useApp();
  const dispatch = useDispatch();
  const cw = app.continue;
  const own = tab === "movies" ? app.movies : app.shows;

  const read = cw.items;
  const items = read.status === "ready" ? read.data : NO_ITEMS;
  const listKey =
    read.status === "ready"
      ? `cw:${items.map(continueKey).join(",")}`
      : read.status === "error"
        ? `cw:error:${read.message}`
        : "cw:loading";
  const revision = read.status === "ready" ? versionOf(read.data) : 0;

  // A failed read is shown with "try again"; the second ask for the same
  // failed list is that, and reads the list again.
  const failed = useRef<string | null>(null);
  const fetchPage = useCallback(
    (offset: number, limit: number): Promise<{ total: number; items: ContinueItemDto[] }> => {
      if (read.status === "ready") {
        return Promise.resolve({ total: read.data.length, items: read.data.slice(offset, offset + limit) });
      }
      if (read.status === "error") {
        if (failed.current === read.message) dispatch({ type: "continue/changed" });
        failed.current = read.message;
        return Promise.reject(new Error(read.message));
      }
      return pending();
    },
    [read, dispatch],
  );

  const selectedId = cw.selected ? continueKey(cw.selected) : null;
  const kind = playingKind(app.player);
  const playingId =
    kind === "movie" && app.movies.playing
      ? `movie:${app.movies.playing.movieId}`
      : kind === "episode" && app.shows.playing
        ? `show:${app.shows.playing.series.seriesId}`
        : null;
  const state = useMemo(
    () => ({
      query: own.query,
      view: own.view,
      sort: own.sort,
      years: own.years,
      menu: null,
      revision,
      selectedId,
      playingId,
    }),
    [own.query, own.view, own.sort, own.years, revision, selectedId, playingId],
  );

  const selected = cw.selected;
  const on = useMemo(
    () => ({
      select: (i: ContinueItemDto) => dispatch({ type: "continue/select", item: i }),
      open: (i: ContinueItemDto) => {
        dispatch({ type: "continue/select", item: i });
        void playContinue(dispatch, i, "ask");
      },
      openSelected: () => {
        if (selected) void playContinue(dispatch, selected, "ask");
      },
      // Typing searches the tab: its own list takes this one's place.
      query: (text: string) =>
        dispatch(tab === "movies" ? { type: "movies/query", text } : { type: "shows/query", text }),
      view: (view: MoviesView) => {
        if (tab === "movies") {
          saveMoviesView(view);
          dispatch({ type: "movies/view", view });
        } else {
          saveVodView(SHOWS_VIEW_STORAGE_KEY, view);
          dispatch({ type: "shows/view", view });
        }
      },
      // No sort and no years: the order is its own (fixedOrder).
      menu: NO_MENU,
      sort: NO_MENU,
      years: NO_MENU,
    }),
    [dispatch, selected, tab],
  );

  const onMenu = useCallback(
    (i: ContinueItemDto, at: MenuAt, fromKeyboard: boolean) =>
      dispatch({
        type: "continue/menu",
        menu: { key: continueKey(i), x: at.x, y: at.y, takeFocus: fromKeyboard },
      }),
    [dispatch],
  );

  if (!shown) return children;
  return (
    <OnDemandList<ContinueItemDto>
      besideRail={besideRail}
      idPrefix="cw"
      // The tab's: what looks for its list (a sheet closing) finds this
      // one where the tab's would be.
      kindClass={tab}
      colsClass="cw-cols"
      unavailableLabel="Continue watching unavailable"
      listKey={listKey}
      fetchPage={fetchPage}
      pages={CONTINUE_PAGES}
      state={state}
      marks={NO_MARKS}
      idOf={continueKey}
      withMarks={withMarks}
      heading="Continue watching"
      countLabel={continueCount}
      placeholder={tab === "movies" ? MOVIE_SEARCH_PLACEHOLDER : SHOW_SEARCH_PLACEHOLDER}
      emptyNote={CONTINUE_EMPTY}
      columns={COLUMNS}
      Row={ContinueRow}
      card={continueCard}
      fixedOrder="Last watched first"
      onMenu={onMenu}
      on={on}
    />
  );
}
