// The Movies tab's rail: the lists that are always there (Favourites, Recently
// added, Watched), then the provider's movie categories, filtered by the box
// at the top and ordered by the button beside it.
//
// It is Live TV's rail with fewer things on it, and made of the same parts:
// `railRows` does the filtering, the match highlight and the ordering, and the
// rows are windowed on the same arithmetic, since a provider's movie
// categories run to the hundreds too. There is nothing to pin or hide here, so
// no options menu and no pin on a row.

import { useEffect, useMemo, useRef, useState } from "react";
import { loadMovieCategories } from "../actions";
import { SORT_LABEL, nextSort, railRows, sortTip } from "../logic/categoryOrder";
import { MOVIE_FAVOURITES, MOVIE_WATCHED, fixedMovieItems } from "../logic/movies";
import { OVERSCAN, ROW_HEIGHT, computeWindow } from "../logic/virtual";
import { useApp, useDispatch } from "../store";
import { Matched } from "./CategoryRail";
import { SortOrder } from "./icons";
import { SearchField } from "./SearchField";

const NO_PINS: readonly string[] = [];

export function MoviesRail() {
  const { movies } = useApp();
  const { summary, categories, selection, categoryFilter, categorySort } = movies;
  const dispatch = useDispatch();
  const scrollerRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [height, setHeight] = useState(0);

  useEffect(() => {
    const el = scrollerRef.current;
    if (!el) return;
    setHeight(el.clientHeight);
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setHeight(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const rows = useMemo(
    () =>
      railRows(
        fixedMovieItems(summary.status === "ready" ? summary.data : null),
        categories.status === "ready"
          ? categories.data.map((c) => ({ id: c.id, name: c.name, channelCount: c.movieCount }))
          : [],
        { pins: NO_PINS, sort: categorySort, filter: categoryFilter },
      ),
    [summary, categories, categorySort, categoryFilter],
  );

  const win = computeWindow(scrollTop, height, ROW_HEIGHT, rows.length, OVERSCAN);

  /** A new order is a different list, and the offset the old one was
   *  scrolled to lands somewhere arbitrary in it. */
  const cycleSort = () => {
    dispatch({ type: "movies/categorySort", sort: nextSort(categorySort) });
    const el = scrollerRef.current;
    if (el) el.scrollTop = 0;
    setScrollTop(0);
  };

  return (
    <>
      <div className="pane-head rail-filter-row">
        <SearchField
          placeholder="Filter categories"
          value={categoryFilter}
          onChange={(text) => dispatch({ type: "movies/categoryFilter", text })}
          clearLabel="Clear filter"
          onKeyDown={(e) => {
            if (e.key === "Escape" && categoryFilter !== "") {
              e.stopPropagation();
              dispatch({ type: "movies/categoryFilter", text: "" });
            }
          }}
        />
        <span className="rail-sort-split">
          <button
            className="rail-sort"
            aria-label={`Sort: ${SORT_LABEL[categorySort].long}`}
            data-tip={sortTip(categorySort)}
            onClick={cycleSort}
          >
            <SortOrder className="rail-sort-i" />
            <span className="rail-sort-label">{SORT_LABEL[categorySort].short}</span>
          </button>
        </span>
      </div>

      <div
        className="scroller rail-scroller"
        ref={scrollerRef}
        onScroll={(e) => {
          const top = e.currentTarget.scrollTop;
          setScrollTop((prev) =>
            Math.floor(top / ROW_HEIGHT) === Math.floor(prev / ROW_HEIGHT) ? prev : top,
          );
        }}
      >
        {categories.status === "loading" ? <div className="pane-note">Loading categories…</div> : null}

        {categories.status === "error" ? (
          <div className="pane-error">
            <span>Categories unavailable — {categories.message}</span>
            <button className="link" onClick={() => void loadMovieCategories(dispatch)}>
              try again
            </button>
          </div>
        ) : null}

        <div className="rail-spacer" style={{ height: win.totalHeight }}>
          {rows.slice(win.start, win.end).map((r, i) => (
            <div
              key={r.id}
              className={`rail-row ${r.kind === "fixed" ? "fixed" : "plain"} ${r.pinEnd ? "pin-end" : ""}`}
              style={{ top: (win.start + i) * ROW_HEIGHT }}
            >
              <button
                className={`rail-item ${r.id === selection ? "sel" : ""}`}
                onClick={() => dispatch({ type: "movies/select", id: r.id })}
              >
                <FixedIcon id={r.id} />
                <span className="rail-name" data-tip-overflow="">
                  <Matched row={r} />
                </span>
                <span className="rail-count">{r.count}</span>
              </button>
            </div>
          ))}
        </div>

        {categories.status === "ready" && !rows.some((r) => r.kind === "category") ? (
          <div className="pane-note">
            {categories.data.length === 0
              ? "No categories yet."
              : `No category matches “${categoryFilter}”.`}
          </div>
        ) : null}
      </div>
    </>
  );
}

/** The mark before a fixed row's name: a star before Favourites, a tick before
 *  Watched. */
function FixedIcon({ id }: { id: string }) {
  if (id === MOVIE_FAVOURITES) return <span className="rail-fixed-i" aria-hidden>★</span>;
  if (id === MOVIE_WATCHED) return <span className="rail-fixed-i" aria-hidden>✓</span>;
  return null;
}
