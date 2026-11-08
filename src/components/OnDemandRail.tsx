// An on-demand tab's rail: the lists that are always there (Favourites,
// Recently added, Watched for Movies), then the provider's categories,
// filtered by the box at the top and ordered by the button beside it. The
// Movies rail made kind-agnostic: the tab supplies the items, the state and
// what a choice does.
//
// It is Live TV's rail with fewer things on it, and made of the same parts:
// `railRows` does the filtering, the match highlight and the ordering, and the
// rows are windowed on the same arithmetic, since a provider's categories run
// to the hundreds too. There is nothing to pin or hide here, so no options
// menu and no pin on a row.

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { Async } from "../logic/async";
import { SORT_LABEL, nextSort, railRows, sortTip, type CategorySort } from "../logic/categoryOrder";
import type { FixedRailItem } from "../logic/railItems";
import { OVERSCAN, ROW_HEIGHT, computeWindow } from "../logic/virtual";
import { Matched } from "./CategoryRail";
import { SortOrder } from "./icons";
import { SearchField } from "./SearchField";

const NO_PINS: readonly string[] = [];

export interface OnDemandRailProps {
  fixed: FixedRailItem[];
  categories: Async<{ id: string; name: string; count: number }[]>;
  selection: string;
  filter: string;
  sort: CategorySort;
  /** The mark before a row's name, if any. */
  icon: (id: string) => ReactNode;
  onSelect: (id: string) => void;
  onFilter: (text: string) => void;
  onSort: (s: CategorySort) => void;
  onRetry: () => void;
}

export function OnDemandRail({
  fixed,
  categories,
  selection,
  filter,
  sort,
  icon,
  onSelect,
  onFilter,
  onSort,
  onRetry,
}: OnDemandRailProps) {
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
        fixed,
        categories.status === "ready"
          ? categories.data.map((c) => ({ id: c.id, name: c.name, channelCount: c.count }))
          : [],
        { pins: NO_PINS, sort, filter },
      ),
    [fixed, categories, sort, filter],
  );

  const win = computeWindow(scrollTop, height, ROW_HEIGHT, rows.length, OVERSCAN);

  /** A new order is a different list, and the offset the old one was
   *  scrolled to lands somewhere arbitrary in it. */
  const cycleSort = () => {
    onSort(nextSort(sort));
    const el = scrollerRef.current;
    if (el) el.scrollTop = 0;
    setScrollTop(0);
  };

  return (
    <>
      <div className="pane-head rail-filter-row">
        <SearchField
          placeholder="Filter categories"
          value={filter}
          onChange={onFilter}
          clearLabel="Clear filter"
          onKeyDown={(e) => {
            if (e.key === "Escape" && filter !== "") {
              e.stopPropagation();
              onFilter("");
            }
          }}
        />
        <span className="rail-sort-split">
          <button
            className="rail-sort"
            aria-label={`Sort: ${SORT_LABEL[sort].long}`}
            data-tip={sortTip(sort)}
            onClick={cycleSort}
          >
            <SortOrder className="rail-sort-i" />
            <span className="rail-sort-label">{SORT_LABEL[sort].short}</span>
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
            <button className="link" onClick={onRetry}>
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
                onClick={() => onSelect(r.id)}
              >
                {icon(r.id)}
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
            {categories.data.length === 0 ? "No categories yet." : `No category matches “${filter}”.`}
          </div>
        ) : null}
      </div>
    </>
  );
}
