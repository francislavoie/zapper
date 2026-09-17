// Hundreds of categories are far too many to scan, so the rail is filterable
// and the filter is local: the backend's nucleo index is for channels, and
// spending a round trip on a few-hundred-item substring match would be silly.
//
// They are also far too many to have as DOM nodes all at once: thousands of
// elements in a scroller tens of thousands of pixels tall, against the few
// dozen the channel list keeps for any number of rows. The scroll handler is
// not the cost; repainting that much of a webview that is composited over the
// video surface is. So the rail windows itself the same way the
// channel list does, through the same arithmetic, and the "All channels"
// entry is simply row zero rather than a special case outside the window.

import { useEffect, useMemo, useRef, useState } from "react";
import { backend } from "../backend";
import { errorMessage, failed, loading, ready } from "../logic/async";
import { OVERSCAN, ROW_HEIGHT, computeWindow } from "../logic/virtual";
import { loadSession, saveSession } from "../session";
import { ALL_CATEGORIES, useApp, useDispatch, type Action } from "../store";
import type { Dispatch } from "react";

export function CategoryRail() {
  const { categories, categoryId, categoryFilter, sync } = useApp();
  const dispatch = useDispatch();
  const requested = useRef(false);
  const scrollerRef = useRef<HTMLDivElement | null>(null);
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

  // Activity keeps this component's state and refs while the tab is hidden
  // but does tear its effects down and build them again, so this runs afresh
  // every time the tab comes back. Both guards matter: the ref survives that
  // cycle and stops a second fetch, and the status check means an already
  // loaded catalogue is never re-fetched even if the ref were lost.
  useEffect(() => {
    if (requested.current || categories.status === "ready") return;
    requested.current = true;
    void load(dispatch);
  }, [dispatch, categories.status]);

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

  const list = useMemo(() => {
    if (categories.status !== "ready") return [];
    const q = categoryFilter.trim().toLowerCase();
    if (q === "") return categories.data;
    return categories.data.filter((c) => c.name.toLowerCase().includes(q));
  }, [categories, categoryFilter]);

  const total =
    categories.status === "ready"
      ? categories.data.reduce((n, c) => n + c.channelCount, 0)
      : sync.channelsTotal;

  /** Row zero is "All channels". Making it part of the list rather than a
   *  sibling above it means one set of offsets, and the window arithmetic
   *  does not need to know it exists. */
  const rows = useMemo(
    () => [
      { id: ALL_CATEGORIES, name: "All channels", count: total, title: undefined as string | undefined },
      ...list.map((c) => ({
        id: c.id,
        name: c.name,
        count: c.channelCount,
        title: c.name as string | undefined,
      })),
    ],
    [list, total],
  );

  const win = computeWindow(scrollTop, height, ROW_HEIGHT, rows.length, OVERSCAN);

  // The `.rail` column itself, and the tab bar above this, belong to Rail:
  // this component is one of the two lists that can be in it.
  return (
    <>
      <div className="pane-head">
        <input
          className="field"
          placeholder="Filter categories"
          value={categoryFilter}
          spellCheck={false}
          onChange={(e) => dispatch({ type: "categoryFilter", text: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === "Escape" && categoryFilter !== "") {
              e.stopPropagation();
              dispatch({ type: "categoryFilter", text: "" });
            }
          }}
        />
        {/* Settings moved to the toolbar's menu; the gear that was here made
            the rail's head a second, competing command surface. */}
      </div>

      <div
        className="scroller"
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
            <button className="link" onClick={() => void load(dispatch)}>
              try again
            </button>
          </div>
        ) : null}

        {categories.status === "ready" && list.length === 0 ? (
          <div className="pane-note">
            {categories.data.length === 0
              ? "No categories yet. Sync has not run."
              : `No category matches “${categoryFilter}”.`}
          </div>
        ) : null}

        <div className="rail-spacer" style={{ height: win.totalHeight }}>
          {rows.slice(win.start, win.end).map((r, i) => {
            const index = win.start + i;
            return (
              <button
                key={r.id}
                className={`rail-item ${r.id === categoryId ? "sel" : ""}`}
                title={r.title}
                style={{ top: index * ROW_HEIGHT }}
                onClick={() => dispatch({ type: "selectCategory", id: r.id })}
              >
                <span className="rail-name">{r.name}</span>
                <span className="rail-count">{r.count ? r.count.toLocaleString() : ""}</span>
              </button>
            );
          })}
        </div>
      </div>
    </>
  );
}

async function load(dispatch: Dispatch<Action>) {
  dispatch({ type: "categories", value: loading() });
  try {
    dispatch({ type: "categories", value: ready(await backend.categories()) });
  } catch (e) {
    dispatch({ type: "categories", value: failed(errorMessage(e)) });
  }
}
