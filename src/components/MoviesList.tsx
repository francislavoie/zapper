// The Movies tab's main pane: the heading row (what is listed, how many, the
// search, sort, years and List | Posters) over the titles themselves.
//
// The titles are paged and windowed like the channel list's — a provider's
// catalogue is a hundred thousand and more — with one scrolling element for
// both views, so the offset, the size observer and the keyboard are not done
// twice. A list is a row per title; the posters are the same titles as cards,
// several to a row of the grid, windowed by grid rows.
//
// Choosing a title selects it (the band's details follow); double-click or
// Enter opens its sheet. Neither plays anything: browsing never touches the
// picture that is on.

import { useCallback, useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { backend } from "../backend";
import { searchInputRegistration } from "../focus";
import { useClock } from "../hooks/useClock";
import { useMovieSearch } from "../hooks/useMovieSearch";
import { usePagedRows } from "../hooks/usePagedRows";
import { PageBook } from "../logic/pagedRows";
import { movieCursorKey, scrollToShow } from "../logic/movieKeys";
import {
  MOVIE_SORTS,
  MOVIE_SORT_LABEL,
  MOVIE_SEARCH_PLACEHOLDER,
  POSTER_GAP,
  movieCount,
  movieQuery,
  movieRailKind,
  movieTitle,
  posterGrid,
  yearRanges,
  type MoviesView,
  type YearRange,
} from "../logic/movies";
import { LIST_ROW_HEIGHT, OVERSCAN, computeWindow } from "../logic/virtual";
import { saveMoviesView } from "../moviesView";
import { useApp, useDispatch } from "../store";
import type { MovieDto } from "../types";
import { ContextMenu } from "./ContextMenu";
import { ChevronDown, Check } from "./icons";
import { MovieRow } from "./MovieRow";
import { PosterGrid } from "./PosterGrid";
import { RailToggle } from "./Rail";
import { SearchField } from "./SearchField";

/** Grid rows drawn above and below the screen's. */
const GRID_OVERSCAN = 1;
const NO_ROWS = () => undefined;

/** The one cache every Movies list draws from: the browse grid's and the one
 *  stacked under the picture show the same titles. */
const MOVIE_PAGES = new PageBook<MovieDto>();
const NOOP = () => {};

const sameRange = (a: YearRange | null, b: YearRange | null) =>
  (a?.from ?? null) === (b?.from ?? null) && (a?.to ?? null) === (b?.to ?? null);

/** `besideRail`: this list shares the browse grid with the rail, so the
 *  heading carries the button that folds it. Stacked under the picture there
 *  is no rail to fold. */
export function MoviesList({ besideRail = true }: { besideRail?: boolean }) {
  const { movies } = useApp();
  const dispatch = useDispatch();
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const sortRef = useRef<HTMLButtonElement | null>(null);
  const yearsRef = useRef<HTMLButtonElement | null>(null);
  const searchRef = useMemo(searchInputRegistration, []);
  const [scrollTop, setScrollTop] = useState(0);
  const [size, setSize] = useState({ width: 0, height: 0 });

  const searching = movies.query.trim() !== "";
  const posters = movies.view === "posters";
  const now = useClock(60_000);

  // ------------------------------------------------------------- the rows

  const query = useMemo(
    () => movieQuery(movies.selection, movies.sort, movies.years),
    [movies.selection, movies.sort, movies.years],
  );
  const listKey = JSON.stringify(query);
  const paged = usePagedRows<MovieDto>(
    listKey,
    (offset, limit) => backend.movies(query, offset, limit),
    movies.revision,
    MOVIE_PAGES,
  );
  const found = useMovieSearch(movies.query);
  const foundRows = found.status === "ready" ? found.data : null;

  const total = searching ? (foundRows?.length ?? 0) : paged.total;
  const loading = searching ? found.status === "loading" : paged.loading;
  const error = searching ? (found.status === "error" ? found.message : null) : paged.error;
  const retry = searching ? NOOP : paged.retry;

  // What a mark has said since the page was fetched, over what the page
  // holds: the rows reload on a mark, but not before the answer is back.
  // The patched row is kept per title, so a marked title is the same object
  // from one render to the next and its memoised row is not redrawn.
  const marks = movies.marks;
  const patched = useRef(new Map<number, MovieDto>());
  const rowAt = useCallback(
    (i: number): MovieDto | undefined => {
      const m = searching ? foundRows?.[i] : paged.rowAt(i);
      const k = m ? marks[m.movieId] : undefined;
      if (!m || !k) return m;
      const had = patched.current.get(m.movieId);
      if (had && had.favourite === k.favourite && had.watched === k.watched && had.name === m.name && had.added === m.added && had.posterUrl === m.posterUrl) {
        return had;
      }
      const next = { ...m, favourite: k.favourite, watched: k.watched };
      patched.current.set(m.movieId, next);
      return next;
    },
    [searching, foundRows, paged.rowAt, marks],
  );
  const ensure = searching ? NOOP : paged.ensure;

  // ------------------------------------------------------------- geometry

  // Measured rather than assumed: the window is resizable, and the number of
  // rows or columns that fit is the point of windowing.
  useEffect(() => {
    const el = viewportRef.current;
    if (!el) return;
    const measure = () => setSize({ width: el.clientWidth, height: el.clientHeight });
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const grid = posterGrid(Math.max(0, size.width - 2 * POSTER_GAP), total);
  const rowStep = posters ? grid.rowHeight : LIST_ROW_HEIGHT;
  const listWin = computeWindow(scrollTop, size.height, LIST_ROW_HEIGHT, total, OVERSCAN);
  const gridWin = computeWindow(scrollTop, size.height, grid.rowHeight, grid.rows, GRID_OVERSCAN);
  const first = posters ? gridWin.start * grid.columns : listWin.start;
  const last = posters ? Math.min(total, gridWin.end * grid.columns) : listWin.end;
  const spacerHeight = posters ? gridWin.totalHeight + (total > 0 ? 2 * POSTER_GAP : 0) : listWin.totalHeight;

  // Only the pages the window touches, and only once each: `ensure` knows
  // which it already holds or has asked for, so a scroll tick is not a request.
  useEffect(() => {
    if (!searching) ensure(first, last);
  }, [searching, ensure, first, last]);

  // A new list starts at the top, and from no row: the old offset would land
  // in the middle of an unrelated one.
  const lists = `${listKey}\u0000${movies.query.trim()}`;
  const lastLists = useRef(lists);
  const cursor = useRef<number | null>(null);
  useEffect(() => {
    if (lastLists.current === lists) return;
    lastLists.current = lists;
    cursor.current = null;
    viewportRef.current?.scrollTo({ top: 0 });
    setScrollTop(0);
  }, [lists]);

  // ------------------------------------------------------------ selection

  const selectedId = movies.selected?.movieId ?? null;
  const onSelect = useCallback(
    (m: MovieDto, index: number) => {
      cursor.current = index;
      dispatch({ type: "movies/selectMovie", movie: m });
    },
    [dispatch],
  );
  const onOpen = useCallback(
    (m: MovieDto, index: number) => {
      cursor.current = index;
      dispatch({ type: "movies/selectMovie", movie: m });
      dispatch({ type: "movies/sheet", id: m.movieId });
    },
    [dispatch],
  );

  /** The index the keys move from: the one last moved to while its row is
   *  still on its way, else wherever the selected title is. */
  const currentIndex = (full = false): number => {
    const c = cursor.current;
    if (c !== null) {
      const at = rowAt(c);
      if (at === undefined || at.movieId === selectedId) return c;
    }
    if (selectedId === null) return -1;
    // Where it was not remembered (selected from elsewhere): the rows on
    // screen, or on request every loaded one. Never the whole catalogue on
    // a key press.
    for (let i = full ? 0 : first; i < (full ? total : last); i++) {
      if (rowAt(i)?.movieId === selectedId) return i;
    }
    return -1;
  };

  // A key may land on a row whose page is still coming; the selection waits
  // for it here, after the render that brings it.
  const waitingFor = useRef<number | null>(null);
  useEffect(() => {
    const i = waitingFor.current;
    if (i === null) return;
    const m = rowAt(i);
    if (!m) return;
    waitingFor.current = null;
    if (m.movieId !== selectedId) dispatch({ type: "movies/selectMovie", movie: m });
  });

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === "Enter") {
      if (movies.selected) {
        e.preventDefault();
        dispatch({ type: "movies/sheet", id: movies.selected.movieId });
      }
      return;
    }
    const el = viewportRef.current;
    if (!el) return;
    const to = movieCursorKey(e.key, {
      current: currentIndex(),
      total,
      columns: posters ? grid.columns : 1,
      visibleRows: Math.max(1, Math.floor(size.height / rowStep)),
      posters,
    });
    if (to === null) return;
    // The player's own arrows must not also fire, nor the page scroll.
    e.preventDefault();
    e.stopPropagation();
    cursor.current = to;
    // At once when its row is here; otherwise when its page arrives.
    const there = rowAt(to);
    if (there) {
      waitingFor.current = null;
      if (there.movieId !== selectedId) dispatch({ type: "movies/selectMovie", movie: there });
    } else {
      waitingFor.current = to;
    }
    const row = posters ? Math.floor(to / grid.columns) : to;
    const top = scrollToShow(
      posters ? POSTER_GAP + row * grid.rowHeight : row * LIST_ROW_HEIGHT,
      rowStep,
      el.scrollTop,
      size.height,
    );
    if (top !== el.scrollTop) el.scrollTo({ top });
    setScrollTop(top);
  };

  // ------------------------------------------------------------- headings

  const categories = movies.categories.status === "ready" ? movies.categories.data : [];
  const heading = searching ? "Search results" : movieTitle(movies.selection, categories);
  const yearChoices = useMemo(() => yearRanges(new Date().getFullYear()), []);
  const yearLabel = yearChoices.find((y) => sameRange(y.range, movies.years))?.label ?? "All years";

  const openMenu = (kind: "sort" | "years", button: HTMLButtonElement | null) => {
    if (movies.menu?.kind === kind) {
      dispatch({ type: "movies/menu", menu: null });
      return;
    }
    const box = button?.getBoundingClientRect();
    if (!box) return;
    dispatch({ type: "movies/menu", menu: { kind, x: box.left, y: box.bottom + 4 } });
  };

  const setView = (view: MoviesView) => {
    if (view === movies.view) return;
    saveMoviesView(view);
    dispatch({ type: "movies/view", view });
    // The offsets mean different things in the two views; the selection is
    // the same title, so it is brought into sight once the new view is laid
    // out (below).
    refit.current = true;
  };

  const refit = useRef(false);
  useEffect(() => {
    if (!refit.current) return;
    refit.current = false;
    const el = viewportRef.current;
    if (!el) return;
    const at = currentIndex(true);
    let top = 0;
    if (at >= 0) {
      const row = posters ? Math.floor(at / grid.columns) : at;
      top = scrollToShow(
        posters ? POSTER_GAP + row * grid.rowHeight : row * LIST_ROW_HEIGHT,
        rowStep,
        0,
        size.height,
      );
    }
    el.scrollTo({ top });
    setScrollTop(top);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [movies.view]);

  // --------------------------------------------------------------- drawing

  // Ids are per list: two lists (the browse grid's, hidden, and the stacked
  // one) can be in the page at once. The active descendant is named only
  // while its row is drawn.
  const idPrefix = `movie${useId()}`;
  let selectedShown = false;
  if (selectedId !== null) {
    for (let i = first; i < last; i++) {
      if (rowAt(i)?.movieId === selectedId) {
        selectedShown = true;
        break;
      }
    }
  }

  const rows = [];
  if (!posters) {
    for (let i = listWin.start; i < listWin.end; i++) {
      const m = rowAt(i);
      const top = i * LIST_ROW_HEIGHT;
      if (!m) {
        rows.push(<div className="row" key={`gap-${i}`} style={{ top, height: LIST_ROW_HEIGHT }} />);
        continue;
      }
      rows.push(
        <MovieRow
          key={m.movieId}
          movie={m}
          top={top}
          index={i}
          idPrefix={idPrefix}
          selected={m.movieId === selectedId}
          now={now}
          onSelect={onSelect}
          onOpen={onOpen}
        />,
      );
    }
  }

  const empty = total === 0 && !loading && !error;

  return (
    <main className="list-pane movies-pane">
      <div className="list-head">
        {besideRail ? <RailToggle /> : null}
        <span className="list-title" data-tip-overflow="">
          {heading}
        </span>
        <span className="list-count">{movieCount(total)}</span>
        {loading ? <span className="list-count">loading…</span> : null}
        <SearchField
          className="list-search"
          inputRef={searchRef}
          placeholder={MOVIE_SEARCH_PLACEHOLDER}
          keyHint="/"
          value={movies.query}
          onChange={(text) => dispatch({ type: "movies/query", text })}
          clearLabel="Clear search"
        />
        <span className="spring" />
        {/* A search lists by how well each title matches, so neither the order
            nor the years apply to it. */}
        {searching ? null : (
          <>
            <button
              className={`movies-menu-btn ${movies.menu?.kind === "sort" ? "on" : ""}`}
              ref={sortRef}
              aria-haspopup="menu"
              aria-expanded={movies.menu?.kind === "sort"}
              aria-label={`Sort: ${MOVIE_SORT_LABEL[movies.sort]}`}
              data-tip="Sort"
              onClick={() => openMenu("sort", sortRef.current)}
            >
              {MOVIE_SORT_LABEL[movies.sort]}
              <ChevronDown className="movies-menu-i" />
            </button>
            <button
              className={`movies-menu-btn ${movies.menu?.kind === "years" ? "on" : ""}`}
              ref={yearsRef}
              aria-haspopup="menu"
              aria-expanded={movies.menu?.kind === "years"}
              aria-label={`Years: ${yearLabel}`}
              data-tip="Years"
              onClick={() => openMenu("years", yearsRef.current)}
            >
              {yearLabel}
              <ChevronDown className="movies-menu-i" />
            </button>
          </>
        )}
        <div className="view-switch" role="tablist" aria-label="View">
          {(
            [
              ["posters", "Posters"],
              ["list", "List"],
            ] as const
          ).map(([view, label]) => (
            <button
              key={view}
              role="tab"
              aria-selected={movies.view === view}
              className={movies.view === view ? "on" : ""}
              onClick={() => setView(view)}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      {posters ? null : (
        <div className="list-cols movie-cols" aria-hidden>
          <span />
          <span>Title</span>
          <span>Year</span>
          <span>Length</span>
          <span>Rating</span>
          <span />
        </div>
      )}

      {error ? (
        <div className="pane-error">
          <span>Movies unavailable — {error}</span>
          <button className="link" onClick={retry}>
            try again
          </button>
        </div>
      ) : null}

      <div
        className={`viewport movies-viewport ${posters ? "posters" : ""} ${selectedId !== null ? "has-sel" : ""}`}
        ref={viewportRef}
        role="listbox"
        aria-label={heading}
        aria-activedescendant={selectedShown ? `${idPrefix}-${selectedId}` : undefined}
        tabIndex={0}
        onKeyDown={onKeyDown}
        onScroll={(e) => {
          const top = e.currentTarget.scrollTop;
          // Quantised to the row grid, as the channel list's is: only which
          // rows exist depends on it, so a change inside a row is a render
          // that would draw the same thing.
          setScrollTop((prev) => (Math.floor(top / rowStep) === Math.floor(prev / rowStep) ? prev : top));
        }}
      >
        <div className="spacer" style={{ height: spacerHeight }}>
          {posters ? (
            <PosterGrid
              geometry={grid}
              rows={{ start: gridWin.start, end: gridWin.end }}
              total={total}
              rowAt={total > 0 ? rowAt : NO_ROWS}
              selectedId={selectedId}
              idPrefix={idPrefix}
              now={now}
              onSelect={onSelect}
              onOpen={onOpen}
            />
          ) : (
            rows
          )}
        </div>
        {empty ? <div className="pane-note">{emptyNote(movies.selection, searching, movies.query, movies.years)}</div> : null}
      </div>

      {movies.menu ? (
        <ContextMenu
          x={movies.menu.x}
          y={movies.menu.y}
          heading={movies.menu.kind === "sort" ? "Sort by" : "Years"}
          onClose={() => dispatch({ type: "movies/menu", menu: null })}
          takeFocus
          anchor={movies.menu.kind === "sort" ? sortRef : yearsRef}
        >
          {movies.menu.kind === "sort"
            ? MOVIE_SORTS.map((sort) => (
                <button
                  key={sort}
                  className="tb-menu-item"
                  role="menuitemradio"
                  aria-checked={sort === movies.sort}
                  onClick={() => dispatch({ type: "movies/sort", sort })}
                >
                  <span className="ctx-check">{sort === movies.sort ? <Check className="ctx-check-i" /> : null}</span>
                  <span className="tb-menu-label">{MOVIE_SORT_LABEL[sort]}</span>
                </button>
              ))
            : yearChoices.map((y) => {
                const on = sameRange(y.range, movies.years);
                return (
                  <button
                    key={y.label}
                    className="tb-menu-item"
                    role="menuitemradio"
                    aria-checked={on}
                    onClick={() => dispatch({ type: "movies/years", range: y.range })}
                  >
                    <span className="ctx-check">{on ? <Check className="ctx-check-i" /> : null}</span>
                    <span className="tb-menu-label">{y.label}</span>
                  </button>
                );
              })}
        </ContextMenu>
      ) : null}
    </main>
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
