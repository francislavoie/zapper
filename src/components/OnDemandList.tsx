// An on-demand tab's main pane: the heading row (what is listed, how many, the
// search, sort, years and List | Posters) over the titles themselves. It is
// the Movies list made kind-agnostic: the model (MoviesList for movies)
// supplies what differs — where the rows come from, what a row and a card
// draw, the words, and what a choice does.
//
// The titles are paged and windowed like the channel list's — a provider's
// catalogue is a hundred thousand and more — with one scrolling element for
// both views, so the offset, the size observer and the keyboard are not done
// twice. A list is a row per title; the posters are the same titles as cards,
// several to a row of the grid, windowed by grid rows.
//
// Choosing a title selects it (the band's details follow); double-click or
// Enter opens its sheet, or, where the model has one, what is beside the list
// (the Shows tab's episode pane). Neither plays anything: browsing never
// touches the picture that is on.

import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ComponentType,
  type KeyboardEvent,
} from "react";
import { searchInputRegistration } from "../focus";
import { useClock } from "../hooks/useClock";
import { usePagedRows } from "../hooks/usePagedRows";
import { movieCursorKey, scrollToShow } from "../logic/movieKeys";
import {
  MOVIE_SORTS,
  MOVIE_SORT_LABEL,
  POSTER_GAP,
  posterGrid,
  yearRanges,
  type MoviesView,
  type PosterCard,
  type YearRange,
} from "../logic/movies";
import { neighbourRows, selectionGone, type GoneCheck, type PageBook } from "../logic/pagedRows";
import { LIST_ROW_HEIGHT, OVERSCAN, computeWindow } from "../logic/virtual";
import type { MoviesMenu } from "../moviesState";
import type { MovieSort } from "../types";
import { ContextMenu } from "./ContextMenu";
import { ChevronDown, Check } from "./icons";
import { PosterGrid } from "./PosterGrid";
import { RailToggle } from "./Rail";
import { SearchField } from "./SearchField";

/** What a list row is given, whatever kind of title it draws. */
export interface VodRowProps<T> {
  item: T;
  top: number;
  index: number;
  /** Unique to the list showing it, so two lists on one page share no ids. */
  idPrefix: string;
  selected: boolean;
  /** The player has this title open (or one of its episodes). */
  playing?: boolean;
  /** Unix seconds, for the NEW badge; moves on slowly, so rows keep their
   *  memoised render between its ticks. */
  now: number;
  onSelect: (t: T, i: number) => void;
  onOpen: (t: T, i: number) => void;
}

type Marks = { favourite: boolean; watched: boolean };

export interface OnDemandListProps<T> {
  /** This list shares the browse grid with the rail, so the heading carries
   *  the button that folds it. Stacked under the picture there is no rail to
   *  fold. */
  besideRail: boolean;
  /** "movie" | "show": the start of the row ids, made unique per list. */
  idPrefix: string;
  /** "movies" | "shows": the pane's and the list's own classes
   *  (`movies-pane`, `movies-viewport`), so each kind's can be told apart
   *  when both are in the page. */
  kindClass: string;
  /** The .list-cols / row grid class. */
  colsClass: string;
  /** What the error line says before the error: "Movies unavailable". */
  unavailableLabel: string;
  /** The query, as JSON: a new one is a new list. */
  listKey: string;
  fetchPage: (offset: number, limit: number) => Promise<{ total: number; items: T[] }>;
  /** Module-level, shared with the stacked copy. */
  pages: PageBook<T>;
  state: {
    query: string;
    view: MoviesView;
    sort: MovieSort;
    years: YearRange | null;
    menu: MoviesMenu | null;
    revision: number;
    selectedId: number | null;
    /** The title the player has open, marked in its row. */
    playingId?: number | null;
  };
  /** The latest marks the service reported, by id, over what a page holds. */
  marks: Readonly<Record<number, Marks>>;
  idOf: (t: T) => number;
  withMarks: (t: T, k: Marks) => T;
  heading: string;
  countLabel: (n: number) => string;
  placeholder: string;
  emptyNote: string;
  /** The .list-cols labels, an empty string for an unlabelled column. */
  columns: readonly string[];
  /** Memoised; it keeps its render while its props do. */
  Row: ComponentType<VodRowProps<T>>;
  /** What a poster card draws; the same function from render to render. */
  card: (t: T, now: number) => PosterCard;
  on: {
    select(t: T): void;
    /** A click on a title, where it does more than the keys' selection
     *  (the Shows tab opens its pane); `select` where absent. */
    pick?(t: T): void;
    open(t: T): void;
    /** → in the list view: into what is beside the list, when there is
     *  something; whether it went. */
    into?(): boolean;
    /** The selected title is not in the list once it has been read again
     *  (a mark, a sync). */
    gone?(): void;
    /** Enter: open the selected title (a movie's sheet, a show's episode pane),
     *  wherever it was selected from (a sheet's shelf picks titles this list
     *  may not hold). */
    openSelected(): void;
    query(text: string): void;
    menu(m: MoviesMenu | null): void;
    view(v: MoviesView): void;
    sort(s: MovieSort): void;
    years(r: YearRange | null): void;
  };
}

/** Grid rows drawn above and below the screen's. */
const GRID_OVERSCAN = 1;
const NO_ROWS = () => undefined;

const sameRange = (a: YearRange | null, b: YearRange | null) =>
  (a?.from ?? null) === (b?.from ?? null) && (a?.to ?? null) === (b?.to ?? null);

/** The same fields, the same values: a page read again holds new objects for
 *  the same titles. */
function sameRow(a: object, b: object): boolean {
  if (a === b) return true;
  const ka = Object.keys(a);
  if (ka.length !== Object.keys(b).length) return false;
  for (const k of ka) {
    if ((a as Record<string, unknown>)[k] !== (b as Record<string, unknown>)[k]) return false;
  }
  return true;
}

export function OnDemandList<T extends object>(p: OnDemandListProps<T>) {
  const { state, on, idOf, withMarks, Row } = p;
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const sortRef = useRef<HTMLButtonElement | null>(null);
  const yearsRef = useRef<HTMLButtonElement | null>(null);
  const searchRef = useMemo(searchInputRegistration, []);
  const [scrollTop, setScrollTop] = useState(0);
  const [size, setSize] = useState({ width: 0, height: 0 });

  const posters = state.view === "posters";
  const now = useClock(60_000);

  // ------------------------------------------------------------- the rows

  const { listKey } = p;
  // A search is part of the list's query (and so of its key): its matches
  // are paged, sorted and windowed as any other list's.
  const paged = usePagedRows<T>(listKey, p.fetchPage, state.revision, p.pages);
  const { total, loading, error, retry, ensure } = paged;

  // What a mark has said since the page was fetched, over what the page
  // holds: the rows reload on a mark, but not before the answer is back.
  // The patched row is kept per title, so a marked title is the same object
  // from one render to the next and its memoised row is not redrawn.
  const marks = p.marks;
  const patched = useRef(new Map<number, { from: T; marks: Marks; row: T }>());
  const rowAt = useCallback(
    (i: number): T | undefined => {
      const m = paged.rowAt(i);
      if (!m) return m;
      const id = idOf(m);
      const k = marks[id];
      if (!k) return m;
      const had = patched.current.get(id);
      if (had && had.marks.favourite === k.favourite && had.marks.watched === k.watched && sameRow(had.from, m)) {
        return had.row;
      }
      const row = withMarks(m, k);
      patched.current.set(id, { from: m, marks: { favourite: k.favourite, watched: k.watched }, row });
      return row;
    },
    [paged.rowAt, marks, idOf, withMarks],
  );

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
    ensure(first, last);
  }, [ensure, first, last]);

  // A new list starts at the top, and from no row: the old offset would land
  // in the middle of an unrelated one. Another search, or another sort of
  // the same one, is another list.
  const lastKey = useRef(listKey);
  const cursor = useRef<number | null>(null);
  useEffect(() => {
    if (lastKey.current === listKey) return;
    lastKey.current = listKey;
    cursor.current = null;
    viewportRef.current?.scrollTo({ top: 0 });
    setScrollTop(0);
  }, [listKey]);

  // ------------------------------------------------------------ selection

  const selectedId = state.selectedId;
  // Read through a ref, so the rows' handlers stay the same functions however
  // often the model hands over a new `on`, and their memoised renders hold.
  const onRef = useRef(on);
  onRef.current = on;
  const onSelect = useCallback((t: T, index: number) => {
    cursor.current = index;
    const on = onRef.current;
    (on.pick ?? on.select)(t);
  }, []);
  const onOpen = useCallback((t: T, index: number) => {
    cursor.current = index;
    onRef.current.open(t);
  }, []);

  /** The index the keys move from: the one last moved to while its row is
   *  still on its way, else wherever the selected title is. */
  const currentIndex = (full = false): number => {
    const c = cursor.current;
    if (c !== null) {
      const at = rowAt(c);
      if (at === undefined || idOf(at) === selectedId) return c;
    }
    if (selectedId === null) return -1;
    // Where it was not remembered (selected from elsewhere): the rows on
    // screen, or on request every loaded one. Never the whole catalogue on
    // a key press.
    for (let i = full ? 0 : first; i < (full ? total : last); i++) {
      const t = rowAt(i);
      if (t !== undefined && idOf(t) === selectedId) return i;
    }
    return -1;
  };

  // The selected title gone from the list once it is read again: a mark
  // that takes it out (unfavourited under Favourites), a sync that dropped
  // it. Where it was is taken when the refresh starts, from the rows still
  // shown; the pages around it are read again, and the answer waits for
  // them (logic/pagedRows.ts `selectionGone`).
  const goneCheck = useRef<GoneCheck | null>(null);
  const seenRevision = useRef(state.revision);
  useEffect(() => {
    if (seenRevision.current === state.revision) return;
    seenRevision.current = state.revision;
    const at = selectedId === null || !on.gone ? -1 : currentIndex();
    goneCheck.current = at >= 0 && selectedId !== null ? { at, id: selectedId, key: listKey } : null;
    if (goneCheck.current) {
      const around = neighbourRows(at);
      paged.ensure(around[0], around[around.length - 1] + 1);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.revision]);
  useEffect(() => {
    const g = goneCheck.current;
    if (!g) return;
    const verdict = selectionGone(
      g,
      { selectedId, key: listKey, loading: paged.loading, error: paged.error, total, fresh: paged.fresh, freshRows: paged.freshRows },
      idOf,
    );
    if (verdict === "wait") return;
    goneCheck.current = null;
    if (verdict === "gone") on.gone?.();
  });

  // A key may land on a row whose page is still coming; the selection waits
  // for it here, after the render that brings it.
  const waitingFor = useRef<number | null>(null);
  useEffect(() => {
    const i = waitingFor.current;
    if (i === null) return;
    const t = rowAt(i);
    if (!t) return;
    waitingFor.current = null;
    if (idOf(t) !== selectedId) on.select(t);
  });

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === "Enter") {
      if (selectedId !== null) {
        e.preventDefault();
        on.openSelected();
      }
      return;
    }
    if (e.key === "ArrowRight" && !posters && on.into?.()) {
      e.preventDefault();
      e.stopPropagation();
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
      if (idOf(there) !== selectedId) on.select(there);
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

  const { heading } = p;
  const yearChoices = useMemo(() => yearRanges(new Date().getFullYear()), []);
  const yearLabel = yearChoices.find((y) => sameRange(y.range, state.years))?.label ?? "All years";

  const openMenu = (kind: "sort" | "years", button: HTMLButtonElement | null) => {
    if (state.menu?.kind === kind) {
      on.menu(null);
      return;
    }
    const box = button?.getBoundingClientRect();
    if (!box) return;
    on.menu({ kind, x: box.left, y: box.bottom + 4 });
  };

  const setView = (view: MoviesView) => {
    if (view === state.view) return;
    on.view(view);
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
  }, [state.view]);

  // --------------------------------------------------------------- drawing

  // Ids are per list: two lists (the browse grid's, hidden, and the stacked
  // one) can be in the page at once. The active descendant is named only
  // while its row is drawn.
  const idPrefix = `${p.idPrefix}${useId()}`;
  let selectedShown = false;
  if (selectedId !== null) {
    for (let i = first; i < last; i++) {
      const t = rowAt(i);
      if (t !== undefined && idOf(t) === selectedId) {
        selectedShown = true;
        break;
      }
    }
  }

  const rows = [];
  if (!posters) {
    for (let i = listWin.start; i < listWin.end; i++) {
      const t = rowAt(i);
      const top = i * LIST_ROW_HEIGHT;
      if (!t) {
        rows.push(<div className="row" key={`gap-${i}`} style={{ top, height: LIST_ROW_HEIGHT }} />);
        continue;
      }
      const id = idOf(t);
      rows.push(
        <Row
          key={id}
          item={t}
          top={top}
          index={i}
          idPrefix={idPrefix}
          selected={id === selectedId}
          playing={id === state.playingId}
          now={now}
          onSelect={onSelect}
          onOpen={onOpen}
        />,
      );
    }
  }

  const empty = total === 0 && !loading && !error;

  return (
    <main className={`list-pane ${p.kindClass}-pane`}>
      <div className="list-head">
        {p.besideRail ? <RailToggle /> : null}
        <span className="list-title" data-tip-overflow="">
          {heading}
        </span>
        <span className="list-count list-total">{p.countLabel(total)}</span>
        {loading ? <span className="list-count">loading…</span> : null}
        <SearchField
          className="list-search"
          inputRef={searchRef}
          placeholder={p.placeholder}
          keyHint="/"
          value={state.query}
          onChange={(text) => on.query(text)}
          clearLabel="Clear search"
        />
        <span className="spring" />
        <button
          className={`movies-menu-btn ${state.menu?.kind === "sort" ? "on" : ""}`}
          ref={sortRef}
          aria-haspopup="menu"
          aria-expanded={state.menu?.kind === "sort"}
          aria-label={`Sort: ${MOVIE_SORT_LABEL[state.sort]}`}
          data-tip="Sort"
          onClick={() => openMenu("sort", sortRef.current)}
        >
          {MOVIE_SORT_LABEL[state.sort]}
          <ChevronDown className="movies-menu-i" />
        </button>
        <button
          className={`movies-menu-btn years-menu ${state.years ? "set" : ""} ${state.menu?.kind === "years" ? "on" : ""}`}
          ref={yearsRef}
          aria-haspopup="menu"
          aria-expanded={state.menu?.kind === "years"}
          aria-label={`Years: ${yearLabel}`}
          data-tip="Years"
          onClick={() => openMenu("years", yearsRef.current)}
        >
          {/* In a narrowed list, "Years" stands in for "All years". */}
          <span className="years-label">{yearLabel}</span>
          <span className="years-short" aria-hidden>
            Years
          </span>
          <ChevronDown className="movies-menu-i" />
        </button>
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
              aria-selected={state.view === view}
              className={state.view === view ? "on" : ""}
              onClick={() => setView(view)}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      {posters ? null : (
        <div className={`list-cols ${p.colsClass}`} aria-hidden>
          {p.columns.map((label, i) => (
            <span key={i}>{label}</span>
          ))}
        </div>
      )}

      {error ? (
        <div className="pane-error">
          <span>
            {p.unavailableLabel} — {error}
          </span>
          <button className="link" onClick={retry}>
            try again
          </button>
        </div>
      ) : null}

      <div
        className={`viewport ${p.kindClass}-viewport ${posters ? "posters" : ""} ${selectedId !== null ? "has-sel" : ""}`}
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
              idOf={idOf}
              card={p.card}
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
        {empty ? <div className="pane-note">{p.emptyNote}</div> : null}
      </div>

      {state.menu ? (
        <ContextMenu
          x={state.menu.x}
          y={state.menu.y}
          heading={state.menu.kind === "sort" ? "Sort by" : "Years"}
          onClose={() => on.menu(null)}
          takeFocus
          anchor={state.menu.kind === "sort" ? sortRef : yearsRef}
        >
          {state.menu.kind === "sort"
            ? MOVIE_SORTS.map((sort) => (
                <button
                  key={sort}
                  className="tb-menu-item"
                  role="menuitemradio"
                  aria-checked={sort === state.sort}
                  onClick={() => on.sort(sort)}
                >
                  <span className="ctx-check">{sort === state.sort ? <Check className="ctx-check-i" /> : null}</span>
                  <span className="tb-menu-label">{MOVIE_SORT_LABEL[sort]}</span>
                </button>
              ))
            : yearChoices.map((y) => {
                const checked = sameRange(y.range, state.years);
                return (
                  <button
                    key={y.label}
                    className="tb-menu-item"
                    role="menuitemradio"
                    aria-checked={checked}
                    onClick={() => on.years(y.range)}
                  >
                    <span className="ctx-check">{checked ? <Check className="ctx-check-i" /> : null}</span>
                    <span className="tb-menu-label">{y.label}</span>
                  </button>
                );
              })}
        </ContextMenu>
      ) : null}
    </main>
  );
}
