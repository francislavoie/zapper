// Hundreds of categories are far too many to scan, so the rail is filterable
// and the filter is local: the backend's nucleo index is for channels, and
// spending a round trip on a few-hundred-item substring match would be silly.
//
// They are also far too many to have as DOM nodes all at once: thousands of
// elements in a scroller tens of thousands of pixels tall, against the few
// dozen the channel list keeps for any number of rows. The scroll handler is
// not the cost; repainting that much of a webview that is composited over the
// video surface is. So the rail windows itself the same way the
// channel list does, through the same arithmetic, and the rail's own items
// (Favourites, Recently watched, Recordings, All channels) are simply the
// first rows rather than a special case outside the window.
//
// Some of those categories are the only ones a given viewer ever opens, so
// they can be pinned: drawn straight under the rail's own items, above a
// hairline, in the order they were pinned. The rest can be re-sorted from the button
// beside the filter box. Others are never wanted at all, so they can be
// hidden, from the same right-click menu; the ▾ beside the sort button brings
// them back, and picks a sort directly. What goes where is worked out in
// logic/categoryOrder.ts; this component only draws it.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { OVERSCAN, ROW_HEIGHT, computeWindow } from "../logic/virtual";
import {
  CATEGORY_SORTS,
  SORT_LABEL,
  hiddenCount,
  nextSort,
  railRows,
  sortTip,
  type CategorySort,
  type RailRow,
} from "../logic/categoryOrder";
import { FAVOURITES, RECENT, RECORDINGS, fixedRailItems } from "../logic/railItems";
import { recordingsRailCount } from "../logic/recordings";
import { loadSession, saveSession } from "../session";
import { loadCategories } from "../actions";
import { useApp, useDispatch } from "../store";
import { ContextMenu } from "./ContextMenu";
import { Check, ChevronDown, EyeOff, Pin, SortOrder } from "./icons";
import { SearchField } from "./SearchField";

export function CategoryRail() {
  const {
    categories,
    categoryId,
    categoryFilter,
    categoryPins,
    categorySort,
    categoryHidden,
    categoryShowHidden,
    categoryMenu,
    categoryOptions,
    sync,
    favourites,
    recent,
    recordings,
  } = useApp();
  const dispatch = useDispatch();
  const scrollerRef = useRef<HTMLDivElement | null>(null);
  const sortRef = useRef<HTMLSpanElement | null>(null);
  const moreRef = useRef<HTMLButtonElement | null>(null);
  const restoreTo = useRef(loadSession().railScrollTop);
  const [scrollTop, setScrollTop] = useState(0);
  const [height, setHeight] = useState(0);

  // Measured, not assumed: the rail is as tall as the window and the number
  // of rows that fit is the whole point of windowing it.
  useEffect(() => {
    const el = scrollerRef.current;
    if (!el) return;
    setHeight(el.clientHeight);
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setHeight(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Applied once the rows exist — before they do there is nothing to scroll,
  // and the assignment is silently clamped to zero.
  useEffect(() => {
    const el = scrollerRef.current;
    const want = restoreTo.current;
    if (want <= 0 || !el || categories.status !== "ready") return;
    restoreTo.current = 0;
    const top = Math.min(want, Math.max(0, el.scrollHeight - el.clientHeight));
    el.scrollTop = top;
    // The window is computed from this, not read from the element, so it has
    // to be told as well or the restored view renders the rows at the top.
    setScrollTop(top);
  }, [categories.status]);

  const total =
    categories.status === "ready"
      ? categories.data.reduce((n, c) => n + c.channelCount, 0)
      : sync.channelsTotal;

  /** The rail's own items lead the list rather than sitting above it, so
   *  there is one set of offsets and the window arithmetic does not need to
   *  know they exist. The pinned block is the same: ordinary rows at ordinary
   *  offsets, with the divider drawn inside the last one. */
  const rows = useMemo(
    () =>
      railRows(
        fixedRailItems({
          favourites: favourites.status === "ready" ? favourites.data.length : null,
          recent: recent.status === "ready" ? recent.data.length : null,
          recordings: recordingsRailCount(recordings ?? []),
          total,
        }),
        categories.status === "ready" ? categories.data : [],
        {
          pins: categoryPins,
          sort: categorySort,
          filter: categoryFilter,
          hidden: categoryHidden,
          showHidden: categoryShowHidden,
        },
      ),
    [
      categories,
      total,
      favourites,
      recent,
      recordings,
      categoryPins,
      categorySort,
      categoryFilter,
      categoryHidden,
      categoryShowHidden,
    ],
  );

  const hiddenN = useMemo(
    () => hiddenCount(categories.status === "ready" ? categories.data : [], categoryHidden),
    [categories, categoryHidden],
  );

  const win = computeWindow(scrollTop, height, ROW_HEIGHT, rows.length, OVERSCAN);

  // Whether the scrollbar is drawn over the rows rather than beside them.
  // A classic scrollbar takes its width out of the box, so nothing can be
  // under it; an overlay one (WebKitGTK under GTK's overlay scrolling, macOS)
  // takes none, and lands on the pin and the count at the rows' right end.
  // Measured rather than assumed per platform: the two differ by a desktop
  // setting, and a scroller that does not scroll has no bar at all.
  const [overlayBar, setOverlayBar] = useState(false);
  useLayoutEffect(() => {
    const el = scrollerRef.current;
    if (!el) return;
    const scrolls = el.scrollHeight > el.clientHeight;
    setOverlayBar(scrolls && el.offsetWidth - el.clientWidth === 0);
  }, [height, rows.length]);

  const closeMenu = useCallback(() => dispatch({ type: "categoryMenu", menu: null }), [dispatch]);
  const closeOptions = useCallback(
    () => dispatch({ type: "categoryOptions", menu: null }),
    [dispatch],
  );

  /** A new order is a different list, and the offset the old one was
   *  scrolled to lands somewhere arbitrary in it; the top is the only place
   *  in the new order that means anything. */
  const setSort = (sort: CategorySort) => {
    dispatch({ type: "categorySort", sort });
    if (sort === categorySort) return;
    const el = scrollerRef.current;
    if (el) el.scrollTop = 0;
    setScrollTop(0);
  };

  /** Under the sort pair, left-aligned with it; ContextMenu keeps it inside
   *  the window. The ▾ takes focus first even when clicked — WebKit does not
   *  focus a clicked button — so closing the menu hands focus back to it. */
  const openOptions = () => {
    const box = (sortRef.current ?? moreRef.current)?.getBoundingClientRect();
    if (!box) return;
    moreRef.current?.focus();
    dispatch({ type: "categoryOptions", menu: { x: box.left, y: box.bottom + 4 } });
  };

  // The `.rail` column itself belongs to Rail; the filter row is the top of it.
  return (
    <>
      <div className="pane-head rail-filter-row">
        <SearchField
          placeholder="Filter categories"
          value={categoryFilter}
          onChange={(text) => dispatch({ type: "categoryFilter", text })}
          clearLabel="Clear filter"
          onKeyDown={(e) => {
            if (e.key === "Escape" && categoryFilter !== "") {
              e.stopPropagation();
              dispatch({ type: "categoryFilter", text: "" });
            }
          }}
        />
        {/* Beside the box it orders the results of, and exactly as tall as
            it — see .rail-sort — so this row stays level with the channel
            list's search row. One button that cycles: three orders is few
            enough to step through, and a click is cheaper than opening a
            list to pick from. The ▾ joined to it is for everything else —
            picking an order outright, and the hidden categories. */}
        <span className="rail-sort-split" ref={sortRef}>
          <button
            className="rail-sort"
            aria-label={`Sort: ${SORT_LABEL[categorySort].long}`}
            data-tip={sortTip(categorySort)}
            onClick={() => setSort(nextSort(categorySort))}
          >
            <SortOrder className="rail-sort-i" />
            <span className="rail-sort-label">{SORT_LABEL[categorySort].short}</span>
          </button>
          <button
            className={`rail-sort-more ${categoryOptions ? "on" : ""}`}
            ref={moreRef}
            aria-label="Category options"
            aria-haspopup="menu"
            aria-expanded={categoryOptions !== null}
            data-tip="Sort and hidden categories"
            onClick={() => (categoryOptions ? closeOptions() : openOptions())}
            onKeyDown={(e) => {
              // Enter and Space arrive as a click; ArrowDown is the menu
              // button's own way in.
              if (e.key !== "ArrowDown" || categoryOptions) return;
              e.preventDefault();
              openOptions();
            }}
          >
            <ChevronDown className="rail-sort-more-i" />
          </button>
        </span>
        {/* Settings moved to the toolbar's menu; the gear that was here made
            the rail's head a second, competing command surface. */}
      </div>

      <div
        className={`scroller rail-scroller ${overlayBar ? "overlay-bar" : ""}`}
        ref={scrollerRef}
        onScroll={(e) => {
          const top = e.currentTarget.scrollTop;
          // Quantised to the row grid: see the note in ChannelList. Only
          // which rows exist depends on this, so a change that does not
          // cross a row boundary is a render that produces identical output.
          setScrollTop((prev) =>
            Math.floor(top / ROW_HEIGHT) === Math.floor(prev / ROW_HEIGHT) ? prev : top,
          );
          // A filtered rail is a short list and its offset means nothing
          // against the full one, which is what comes back next launch.
          if (categoryFilter.trim() === "") {
            saveSession({ railScrollTop: top });
          }
        }}
      >
        {categories.status === "loading" ? (
          <div className="pane-note">Loading categories…</div>
        ) : null}

        {categories.status === "error" ? (
          <div className="pane-error">
            <span>Categories unavailable — {categories.message}</span>
            <button className="link" onClick={() => void loadCategories(dispatch)}>
              try again
            </button>
          </div>
        ) : null}

        <div className="rail-spacer" style={{ height: win.totalHeight }}>
          {rows.slice(win.start, win.end).map((r, i) => {
            const index = win.start + i;
            if (r.kind === "hint") {
              return (
                <div
                  key={r.id}
                  className="rail-row rail-hint"
                  style={{ top: index * ROW_HEIGHT }}
                >
                  <button
                    className="rail-item"
                    onClick={() => dispatch({ type: "categoryShowHidden", on: true })}
                  >
                    <span className="rail-name">{r.name}</span>
                  </button>
                </div>
              );
            }
            // The rail's own items are not categories: nothing to pin or hide.
            const pinnable = r.kind === "category";
            return (
              // A box holding two buttons rather than one button, because
              // the pin is a second thing to press and HTML does not allow a
              // button inside a button.
              <div
                key={r.id}
                className={`rail-row ${r.kind === "fixed" ? "fixed" : ""} ${r.pinned ? "pinned" : ""} ${r.pinEnd ? "pin-end" : ""} ${r.hidden ? "is-hidden" : ""}`}
                style={{ top: index * ROW_HEIGHT }}
              >
                <button
                  className={`rail-item ${r.id === categoryId ? "sel" : ""}`}
                  onClick={() =>
                    r.id === RECORDINGS
                      ? dispatch({ type: "showRecordings" })
                      : dispatch({ type: "selectCategory", id: r.id })
                  }
                  onContextMenu={(e) => {
                    if (!pinnable) return;
                    // As in the favourites: WebKit's own menu is suppressed
                    // at the GTK level, so this is for the browser mock.
                    e.preventDefault();
                    dispatch({
                      type: "categoryMenu",
                      menu: { id: r.id, name: r.name, x: e.clientX, y: e.clientY, keyboard: false },
                    });
                  }}
                  onKeyDown={(e) => {
                    // The keyboard's way to the same menu, opened under the
                    // row since there is no pointer to open it at.
                    if (!pinnable) return;
                    if (e.key !== "ContextMenu" && !(e.shiftKey && e.key === "F10")) return;
                    e.preventDefault();
                    const box = e.currentTarget.getBoundingClientRect();
                    dispatch({
                      type: "categoryMenu",
                      menu: { id: r.id, name: r.name, x: box.left + 12, y: box.bottom, keyboard: true },
                    });
                  }}
                >
                  <FixedIcon id={r.id} />
                  {r.pinned ? <Pin className="rail-pinned" filled /> : null}
                  {r.hidden ? <EyeOff className="rail-hidden-i" /> : null}
                  <span className="rail-name" data-tip-overflow="">
                    <Matched row={r} />
                    {r.hidden ? <span className="rail-hidden-note"> (hidden)</span> : null}
                  </span>
                  <span className="rail-count">{r.count}</span>
                </button>
                {pinnable ? (
                  // Over the count, and only while the row is hovered or the
                  // keyboard is on it: the count is what the row says the
                  // rest of the time. Always in the tab order, so pinning
                  // does not need a pointer.
                  <button
                    className="rail-pin"
                    aria-label={r.pinned ? `Unpin ${r.name}` : `Pin ${r.name} to the top`}
                    data-tip={r.pinned ? "Unpin" : "Pin to the top"}
                    onClick={() => dispatch({ type: "categoryPin", id: r.id, pinned: !r.pinned })}
                  >
                    <Pin className="rail-pin-i" filled={r.pinned} />
                  </button>
                ) : null}
              </div>
            );
          })}
        </div>

        {/* No category rows: nothing matched, pinned or not. */}
        {categories.status === "ready" && !rows.some((r) => r.kind === "category") ? (
          <div className="pane-note">
            {categories.data.length === 0
              ? "No categories yet. Sync has not run."
              : categoryFilter.trim() !== ""
                ? `No category matches “${categoryFilter}”.`
                : "Every category is hidden."}
          </div>
        ) : null}
      </div>

      {categoryMenu ? (
        <ContextMenu
          x={categoryMenu.x}
          y={categoryMenu.y}
          heading={categoryMenu.name}
          onClose={closeMenu}
          takeFocus={categoryMenu.keyboard}
        >
          <button
            className="tb-menu-item"
            role="menuitem"
            onClick={() =>
              dispatch({
                type: "categoryPin",
                id: categoryMenu.id,
                pinned: !categoryPins.includes(categoryMenu.id),
              })
            }
          >
            <span className="tb-menu-label">
              {categoryPins.includes(categoryMenu.id) ? "Unpin from the top" : "Pin to the top"}
            </span>
          </button>
          <button
            className="tb-menu-item"
            role="menuitem"
            onClick={() =>
              dispatch({
                type: "categoryHide",
                id: categoryMenu.id,
                hidden: !categoryHidden.includes(categoryMenu.id),
              })
            }
          >
            <span className="tb-menu-label">
              {categoryHidden.includes(categoryMenu.id) ? "Unhide category" : "Hide category"}
            </span>
          </button>
        </ContextMenu>
      ) : null}

      {categoryOptions ? (
        <ContextMenu
          x={categoryOptions.x}
          y={categoryOptions.y}
          heading="Sort by"
          onClose={closeOptions}
          takeFocus
          anchor={moreRef}
        >
          {CATEGORY_SORTS.map((sort) => (
            <button
              key={sort}
              className="tb-menu-item"
              role="menuitemradio"
              aria-checked={sort === categorySort}
              onClick={() => setSort(sort)}
            >
              <span className="ctx-check">
                {sort === categorySort ? <Check className="ctx-check-i" /> : null}
              </span>
              <span className="tb-menu-label">{SORT_LABEL[sort].menu}</span>
            </button>
          ))}
          <div className="ctx-sep" role="separator" />
          <button
            className="tb-menu-item"
            role="menuitemcheckbox"
            aria-checked={categoryShowHidden}
            onClick={() => dispatch({ type: "categoryShowHidden", on: !categoryShowHidden })}
          >
            <span className="ctx-check">
              {categoryShowHidden ? <Check className="ctx-check-i" /> : null}
            </span>
            <span className="tb-menu-label">Show hidden categories</span>
          </button>
          {hiddenN > 0 ? (
            <button
              className="tb-menu-item"
              role="menuitem"
              onClick={() => dispatch({ type: "categoryUnhideAll" })}
            >
              <span className="ctx-check" />
              <span className="tb-menu-label">Unhide all ({hiddenN.toLocaleString()})</span>
            </button>
          ) : null}
        </ContextMenu>
      ) : null}
    </>
  );
}

/** The mark before a fixed row's name: a star, a clock-back arrow, the red
 *  recording dot; All channels has none. Every other row has nothing. */
function FixedIcon({ id }: { id: string }) {
  if (id === FAVOURITES) return <span className="rail-fixed-i" aria-hidden>★</span>;
  if (id === RECENT) return <span className="rail-fixed-i" aria-hidden>↺</span>;
  if (id === RECORDINGS) {
    return (
      <span className="rail-fixed-i" aria-hidden>
        <span className="rec-dot" />
      </span>
    );
  }
  return null;
}

/** The row's name, with the filter's match picked out. */
function Matched({ row }: { row: RailRow }) {
  if (!row.match) return <>{row.name}</>;
  const [from, to] = row.match;
  return (
    <>
      {row.name.slice(0, from)}
      <mark>{row.name.slice(from, to)}</mark>
      {row.name.slice(to)}
    </>
  );
}
