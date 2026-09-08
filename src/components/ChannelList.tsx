// The virtualized list. Tens of thousands of rows, ~47 DOM nodes.
//
// Rows are absolutely positioned inside a spacer of the full height, so the
// scrollbar is honest about how much list there is while only the visible
// window exists. Windowing arithmetic lives in logic/virtual.ts and is
// checked there; this file is the wiring.

import { useCallback, useEffect, useRef, useState } from "react";
import { playChannel } from "../actions";
import { registerSearchInput } from "../focus";
import { useChannelPages, type RowSource } from "../hooks/useChannelPages";
import { useSearch } from "../hooks/useSearch";
import { OVERSCAN, ROW_HEIGHT, computeWindow, stickyIndex } from "../logic/virtual";
import { ALL_CATEGORIES, useApp, useDispatch } from "../store";
import type { Async } from "../logic/async";
import type { ChannelDto } from "../types";
import { ChannelRow } from "./ChannelRow";

/** The backend caps search at a sane number; 100 is plenty to scan. */
const SEARCH_LIMIT = 100;

/** Stable identities so the window effect does not re-run every render. */
const NOOP = () => {};
const NOOP_ENSURE = () => {};

export function ChannelList() {
  const state = useApp();
  const dispatch = useDispatch();
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [height, setHeight] = useState(0);

  const searching = state.query.trim() !== "";
  const categoryId = state.categoryId === ALL_CATEGORIES ? null : state.categoryId;

  const estimate =
    state.categories.status === "ready"
      ? categoryId === null
        ? state.categories.data.reduce((n, c) => n + c.channelCount, 0)
        : (state.categories.data.find((c) => c.id === categoryId)?.channelCount ?? 0)
      : state.sync.channelsTotal;

  const paged = useChannelPages(categoryId, estimate);
  // `/` and Ctrl+P run through the same backend index; the difference is only
  // where the results are shown.
  const results = useSearch(state.query, SEARCH_LIMIT);
  const source: RowSource = searching ? asSource(results) : paged;

  // Measure rather than assume: the window is resizable and the row count per
  // screen is the whole point of the dense layout.
  useEffect(() => {
    const el = viewportRef.current;
    if (!el) return;
    setHeight(el.clientHeight);
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setHeight(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // A new list starts at the top; keeping the old offset would land the user
  // in the middle of an unrelated category.
  useEffect(() => {
    viewportRef.current?.scrollTo({ top: 0 });
    setScrollTop(0);
  }, [state.categoryId, state.query]);

  const win = computeWindow(scrollTop, height, ROW_HEIGHT, source.total, OVERSCAN);

  const { ensure } = source;
  useEffect(() => {
    if (win.end > win.start) ensure(win.start, win.end);
  }, [ensure, win.start, win.end]);

  const onPlay = useCallback(
    (c: ChannelDto) => void playChannel(dispatch, c),
    [dispatch],
  );

  const firstVisible = Math.floor(scrollTop / ROW_HEIGHT);
  const stickyAt = stickyIndex((i) => source.rowAt(i)?.isSeparator, firstVisible);
  // Only pin a header once its own row has scrolled out of sight; otherwise
  // there would be two copies of it on screen.
  const sticky =
    stickyAt !== null && stickyAt * ROW_HEIGHT < scrollTop
      ? source.rowAt(stickyAt)
      : undefined;

  const rows = [];
  for (let i = win.start; i < win.end; i++) {
    const c = source.rowAt(i);
    const top = i * ROW_HEIGHT;
    if (!c) {
      rows.push(<div className="row" key={`gap-${i}`} style={{ top, height: ROW_HEIGHT }} />);
      continue;
    }
    rows.push(
      <ChannelRow
        key={c.streamId}
        channel={c}
        top={top}
        selected={state.player.streamId === c.streamId}
        pending={state.pendingStreamId === c.streamId}
        onPlay={onPlay}
      />,
    );
  }

  return (
    <main className="list-pane">
      <div className="pane-head">
        <input
          className="field"
          ref={registerSearchInput}
          placeholder="Search channels    /"
          spellCheck={false}
          value={state.query}
          onChange={(e) => dispatch({ type: "query", text: e.target.value })}
        />
      </div>
      <ListHead searching={searching} total={source.total} loading={source.loading} />

      {source.error ? (
        <div className="pane-error">
          <span>Channels unavailable — {source.error}</span>
          <button className="link" onClick={source.retry}>
            try again
          </button>
        </div>
      ) : null}

      <div
        className="viewport"
        ref={viewportRef}
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
      >
        <div className="spacer" style={{ height: win.totalHeight }}>
          {rows}
        </div>
        {sticky ? <div className="sticky-head">{sticky.cleanName}</div> : null}
        {source.total === 0 && !source.loading && !source.error ? (
          <div className="pane-note">
            {searching ? `Nothing matches “${state.query}”.` : "No channels here."}
          </div>
        ) : null}
      </div>
    </main>
  );
}

function ListHead({
  searching,
  total,
  loading,
}: {
  searching: boolean;
  total: number;
  loading: boolean;
}) {
  return (
    <div className="list-meta">
      <span>{searching ? "Search results" : "Channels"}</span>
      <span>{total.toLocaleString()}</span>
      {loading ? <span>loading…</span> : null}
    </div>
  );
}

/** Search results are a plain array — no paging, the backend caps the count. */
function asSource(r: Async<ChannelDto[]>): RowSource {
  const data = r.status === "ready" ? r.data : [];
  return {
    total: data.length,
    rowAt: (i) => data[i],
    ensure: NOOP_ENSURE,
    error: r.status === "error" ? r.message : null,
    loading: r.status === "loading",
    retry: NOOP,
  };
}
