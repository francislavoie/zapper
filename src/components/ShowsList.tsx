// The Shows tab's main pane: OnDemandList with the show model — where the
// shows come from, what a row and a card draw, the words, and what a choice
// does. Everything here is memoised so the list's rows keep their renders.

import { useCallback, useMemo } from "react";
import { backend } from "../backend";
import { useShowSearch } from "../hooks/useShowSearch";
import type { MoviesView, YearRange } from "../logic/movies";
import { PageBook } from "../logic/pagedRows";
import {
  SHOW_SEARCH_PLACEHOLDER,
  SHOWS_VIEW_STORAGE_KEY,
  showCard,
  showCount,
  showQuery,
  showRailKind,
  showTitle,
} from "../logic/shows";
import type { MoviesMenu } from "../moviesState";
import { saveVodView } from "../moviesView";
import { useApp, useDispatch } from "../store";
import type { MovieSort, SeriesDto } from "../types";
import { OnDemandList } from "./OnDemandList";
import { ShowRow } from "./ShowRow";

/** The one cache every Shows list draws from: the browse grid's and the one
 *  stacked under the picture show the same shows. */
const SHOW_PAGES = new PageBook<SeriesDto>();

// The first column is the poster's, unlabelled (as the Movies list's).
const SHOW_COLUMNS = ["", "Title", "Year", "Seasons", "Rating", ""] as const;
const idOf = (s: SeriesDto) => s.seriesId;
const withMarks = (s: SeriesDto, k: { favourite: boolean; watched: boolean }): SeriesDto => ({
  ...s,
  favourite: k.favourite,
  watched: k.watched,
});

/** `besideRail`: this list shares the browse grid with the rail, so the
 *  heading carries the button that folds it. Stacked under the picture there
 *  is no rail to fold. */
export function ShowsList({ besideRail = true }: { besideRail?: boolean }) {
  const { shows } = useApp();
  const dispatch = useDispatch();

  const query = useMemo(
    () => showQuery(shows.selection, shows.sort, shows.years),
    [shows.selection, shows.sort, shows.years],
  );
  const listKey = JSON.stringify(query);
  const fetchPage = useCallback(
    (offset: number, limit: number) => backend.series(query, offset, limit),
    [query],
  );
  const search = useShowSearch(shows.query);

  const searching = shows.query.trim() !== "";
  const categories = shows.categories.status === "ready" ? shows.categories.data : [];
  const heading = searching ? "Search results" : showTitle(shows.selection, categories);

  const selectedId = shows.selected?.seriesId ?? null;
  const state = useMemo(
    () => ({
      query: shows.query,
      view: shows.view,
      sort: shows.sort,
      years: shows.years,
      menu: shows.menu,
      revision: shows.revision,
      selectedId,
    }),
    [shows.query, shows.view, shows.sort, shows.years, shows.menu, shows.revision, selectedId],
  );

  const selected = shows.selected;
  const on = useMemo(
    () => ({
      select: (s: SeriesDto) => dispatch({ type: "shows/selectShow", show: s }),
      open: (s: SeriesDto) => {
        dispatch({ type: "shows/selectShow", show: s });
        dispatch({ type: "shows/sheet", id: s.seriesId });
      },
      // Enter opens the selected show, wherever it was chosen.
      openSelected: () => {
        if (selected) dispatch({ type: "shows/sheet", id: selected.seriesId });
      },
      query: (text: string) => dispatch({ type: "shows/query", text }),
      menu: (menu: MoviesMenu | null) => dispatch({ type: "shows/menu", menu }),
      view: (view: MoviesView) => {
        saveVodView(SHOWS_VIEW_STORAGE_KEY, view);
        dispatch({ type: "shows/view", view });
      },
      sort: (sort: MovieSort) => dispatch({ type: "shows/sort", sort }),
      years: (range: YearRange | null) => dispatch({ type: "shows/years", range }),
    }),
    [dispatch, selected],
  );

  return (
    <OnDemandList<SeriesDto>
      besideRail={besideRail}
      idPrefix="show"
      kindClass="shows"
      colsClass="show-cols"
      unavailableLabel="Shows unavailable"
      listKey={listKey}
      fetchPage={fetchPage}
      pages={SHOW_PAGES}
      search={search}
      state={state}
      marks={shows.seriesMarks}
      idOf={idOf}
      withMarks={withMarks}
      heading={heading}
      countLabel={showCount}
      placeholder={SHOW_SEARCH_PLACEHOLDER}
      emptyNote={emptyNote(shows.selection, searching, shows.query, shows.years)}
      columns={SHOW_COLUMNS}
      Row={ShowRow}
      card={showCard}
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
  if (years) return "No shows from those years.";
  switch (showRailKind(selection)) {
    case "favourites":
      return "No favourite shows yet. ★ one to keep it here.";
    case "recent":
      return "Nothing added in the last 30 days.";
    case "watched":
      return "No show watched yet.";
    default:
      return "This category is empty.";
  }
}
