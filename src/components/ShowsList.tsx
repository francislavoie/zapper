// The Shows tab's main pane: OnDemandList with the show model — where the
// shows come from, what a row and a card draw, the words, and what a choice
// does — and beside it the episode pane (ShowPane) on the show selected: a
// click opens it, Enter or → go into its episodes. Stacked under the
// picture, the pane takes the list's place while it is open. Everything here
// is memoised so the list's rows keep their renders.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { backend } from "../backend";
import { useSettledSearch } from "../hooks/useSettledSearch";
import { playingKind } from "../logic/movies";
import { paneKey } from "../logic/showPane";
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
import { isTitleSearch } from "../logic/titleMatch";
import type { MoviesMenu } from "../moviesState";
import { saveVodView } from "../moviesView";
import { useApp, useDispatch } from "../store";
import type { MovieSort, SeriesDto } from "../types";
import { OnDemandList } from "./OnDemandList";
import { ShowPane } from "./ShowPane";
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
  const { shows, player } = useApp();
  const dispatch = useDispatch();
  const split = useRef<HTMLDivElement | null>(null);
  // The show whose episodes Enter or → asked to go into: the pane takes
  // focus once they are there, for that show only. Another selection, the
  // pane closing, or the episodes failing to load drop the ask.
  const [wantsFocus, setWantsFocus] = useState<number | null>(null);
  const settled = useCallback(() => setWantsFocus(null), []);
  const stacked = !besideRail;

  const searched = useSettledSearch(shows.query);
  const query = useMemo(
    () => showQuery(shows.selection, shows.sort, shows.years, searched),
    [shows.selection, shows.sort, shows.years, searched],
  );
  const listKey = JSON.stringify(query);
  const fetchPage = useCallback(
    (offset: number, limit: number) => backend.series(query, offset, limit),
    [query],
  );

  // What the list shows, not what is being typed: the two differ for
  // the moment of the pause.
  const searching = isTitleSearch(searched);
  const categories = shows.categories.status === "ready" ? shows.categories.data : [];
  const heading = searching ? "Search results" : showTitle(shows.selection, categories);

  const selectedId = shows.selected?.seriesId ?? null;
  const playingId =
    playingKind(player) === "episode" ? (shows.playing?.series.seriesId ?? null) : null;
  const state = useMemo(
    () => ({
      query: shows.query,
      view: shows.view,
      sort: shows.sort,
      years: shows.years,
      menu: shows.menu,
      revision: shows.revision,
      selectedId,
      playingId,
    }),
    [shows.query, shows.view, shows.sort, shows.years, shows.menu, shows.revision, selectedId, playingId],
  );

  const selected = shows.selected;
  const paneOpen = shows.paneOpen && selected !== null;
  const posters = shows.view === "posters";
  const on = useMemo(() => {
    // Enter (or → in the list view) goes into the episodes, opening the
    // pane if it is closed (logic/showPane.ts).
    const into = (key: string): boolean => {
      const move = paneKey(paneOpen ? "list" : "closed", key, { selected: selected !== null, posters });
      if (move?.zone !== "episodes" || !selected) return false;
      if (!paneOpen) dispatch({ type: "shows/pane", open: true });
      setWantsFocus(selected.seriesId);
      return true;
    };
    return {
      select: (s: SeriesDto) => {
        setWantsFocus(null);
        dispatch({ type: "shows/selectShow", show: s });
      },
      // A click opens the pane on it.
      pick: (s: SeriesDto) => {
        setWantsFocus(null);
        dispatch({ type: "shows/selectShow", show: s, pane: true });
      },
      // The show the pane is on has left the list (unfavourited under
      // Favourites, dropped by a sync): the pane goes with it.
      gone: () => dispatch({ type: "shows/pane", open: false }),
      open: (s: SeriesDto) => {
        dispatch({ type: "shows/selectShow", show: s });
        dispatch({ type: "shows/sheet", id: s.seriesId });
      },
      openSelected: () => {
        into("Enter");
      },
      into: () => into("ArrowRight"),
      query: (text: string) => dispatch({ type: "shows/query", text }),
      menu: (menu: MoviesMenu | null) => dispatch({ type: "shows/menu", menu }),
      view: (view: MoviesView) => {
        saveVodView(SHOWS_VIEW_STORAGE_KEY, view);
        dispatch({ type: "shows/view", view });
      },
      sort: (sort: MovieSort) => dispatch({ type: "shows/sort", sort }),
      years: (range: YearRange | null) => dispatch({ type: "shows/years", range }),
    };
  }, [dispatch, selected, paneOpen, posters]);

  useEffect(() => {
    if (!paneOpen) setWantsFocus(null);
  }, [paneOpen]);

  const toList = useCallback(() => {
    const list = split.current?.querySelector<HTMLElement>(".shows-viewport");
    // Not a list that is out of sight (another tab, the other layout).
    if (list && list.offsetParent !== null) list.focus();
    else if (list && stacked) requestAnimationFrame(() => list.focus());
  }, [stacked]);

  return (
    <div
      className={`list-split shows-split ${paneOpen ? "pane-open" : ""} ${stacked ? "stacked" : ""}`}
      ref={split}
    >
    <OnDemandList<SeriesDto>
      besideRail={besideRail}
      idPrefix="show"
      kindClass="shows"
      colsClass="show-cols"
      unavailableLabel="Shows unavailable"
      listKey={listKey}
      fetchPage={fetchPage}
      pages={SHOW_PAGES}
      state={state}
      marks={shows.seriesMarks}
      idOf={idOf}
      withMarks={withMarks}
      heading={heading}
      countLabel={showCount}
      placeholder={SHOW_SEARCH_PLACEHOLDER}
      emptyNote={emptyNote(shows.selection, searching, searched, shows.years)}
      columns={SHOW_COLUMNS}
      Row={ShowRow}
      card={showCard}
      on={on}
    />
    {paneOpen && selected ? (
      <ShowPane
        key={stacked ? "stacked" : "beside"}
        show={selected}
        stacked={stacked}
        listName={heading}
        wantsFocus={wantsFocus === selected.seriesId}
        settled={settled}
        toList={toList}
      />
    ) : null}
    </div>
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
