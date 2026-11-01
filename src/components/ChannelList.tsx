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
import { loadSession, saveSession } from "../session";
import { ALL_CATEGORIES, useApp, useDispatch } from "../store";
import type { Async } from "../logic/async";
import type { ChannelDto } from "../types";
import { ChannelRow } from "./ChannelRow";
import { ViewSwitch } from "./ViewSwitch";

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
  /** Where we were last time, until it has been applied. Zeroed once used,
   *  so a later category change resets to the top like any other. */
  const restoreTo = useRef(loadSession().channelScrollTop);
  /** What the list was showing last time the reset effect ran. Seeded with
   *  the *current* value rather than null, so the run on mount sees no
   *  change and leaves the restore below alone.
   *
   *  A "have I run yet" flag does not work here: StrictMode invokes effects
   *  twice, and the second pass would find the flag set, read it as a real
   *  category change, and scroll the remembered position away. */
  const lastKey = useRef(`${state.categoryId}\u0000${state.query}`);

  const searching = state.query.trim() !== "";
  const categoryId = state.categoryId === ALL_CATEGORIES ? null : state.categoryId;
  const categoryName =
    categoryId === null
      ? "All channels"
      : state.categories.status === "ready"
        ? (state.categories.data.find((c) => c.id === categoryId)?.name ?? "Channels")
        : "Channels";

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
  // in the middle of an unrelated category. The first run is not a new list
  // though — it is the one we were on — so it is left to the restore below.
  useEffect(() => {
    const key = `${state.categoryId}\u0000${state.query}`;
    if (lastKey.current === key) return;
    lastKey.current = key;
    restoreTo.current = 0;
    viewportRef.current?.scrollTo({ top: 0 });
    setScrollTop(0);
  }, [state.categoryId, state.query]);

  // Restoring has to wait for two things that arrive later than the mount:
  // the viewport's height, and a row count big enough to scroll to. Setting
  // scrollTop before the spacer is tall enough is silently clamped to zero,
  // which looks exactly like not having remembered.
  useEffect(() => {
    const want = restoreTo.current;
    const el = viewportRef.current;
    if (want <= 0 || !el || height <= 0 || source.total <= 0) return;
    const top = Math.min(want, Math.max(0, source.total * ROW_HEIGHT - height));
    restoreTo.current = 0;
    el.scrollTo({ top });
    setScrollTop(top);
  }, [height, source.total]);

  const win = computeWindow(scrollTop, height, ROW_HEIGHT, source.total, OVERSCAN);

  const { ensure } = source;
  useEffect(() => {
    if (win.end > win.start) ensure(win.start, win.end);
  }, [ensure, win.start, win.end]);

  // Read through a ref so both callbacks keep a stable identity. They are
  // handed to a memoised row, and a callback that changed whenever playback
  // did would re-render every visible row on every channel change.
  const playing = useRef({ pending: state.pendingStreamId, current: state.player.streamId });
  playing.current = { pending: state.pendingStreamId, current: state.player.streamId };

  const isAlready = (c: ChannelDto) =>
    playing.current.pending === c.streamId || playing.current.current === c.streamId;

  // A click starts the channel and leaves you in the list. It does not open
  // the player: the picture is docked top-right and carries on while you
  // keep looking, which is the whole point of the band.
  const onPlay = useCallback((c: ChannelDto) => {
    // Re-clicking what is already on would tear down the connection and
    // build it again for no change. It also makes a double-click safe: the
    // second click of one lands here after the first has already started it.
    if (isAlready(c)) return;
    void playChannel(dispatch, c);
  }, [dispatch]);

  // A double-click means "and show me it". The click that preceded it has
  // already started the channel, so this only has to take the window — but
  // it starts the channel too, for the case where the row was already the
  // one playing and the click above deliberately did nothing.
  const onOpenFull = useCallback((c: ChannelDto) => {
    if (!isAlready(c)) void playChannel(dispatch, c, { full: true });
    else dispatch({ type: "mode", mode: "watch" });
  }, [dispatch]);

  const firstVisible = Math.floor(scrollTop / ROW_HEIGHT);
  const stickyAt = stickyIndex((i) => source.rowAt(i)?.isSeparator, firstVisible);
  // Only pin a header once its own row has scrolled out of sight; otherwise
  // there would be two copies of it on screen.
  // Compared by row rather than by pixel, because `scrollTop` is now only
  // accurate to the row grid. It also removes a flicker: pinning the moment
  // a single pixel of the separator went under the top edge meant the header
  // appeared and disappeared as you nudged across the boundary.
  const sticky =
    stickyAt !== null && stickyAt < firstVisible
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
        onOpenFull={onOpenFull}
      />,
    );
  }

  return (
    <main className="list-pane">
      {/* Two rows, matching the rail's beside it row for row: a heading the
          height of the rail's tabs, then the search box the height of the
          rail's filter box, so the two inputs sit on one line. A full-width search box above a
          count line would put the search level with the rail's tabs and the
          filter a row lower. */}
      <ListHead
        title={searching ? "Search results" : categoryName}
        total={source.total}
        loading={source.loading}
      />
      <div className="pane-head">
        <input
          className="field list-search"
          ref={registerSearchInput}
          placeholder="Search channels    /"
          spellCheck={false}
          value={state.query}
          onChange={(e) => dispatch({ type: "query", text: e.target.value })}
        />
      </div>

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
        onScroll={(e) => {
          const top = e.currentTarget.scrollTop;
          // Quantised to the row grid, and this is the difference between a
          // list that scrolls and one that drags. Nothing rendered here
          // depends on where inside a row the viewport sits: the rows are
          // absolutely positioned at index * ROW_HEIGHT, so only *which*
          // rows exist changes. Storing the raw offset re-rendered the whole
          // window on every scroll event — dozens a second, each one
          // reconciling forty-odd rows to produce identical output. Keeping
          // the previous value when the row has not changed makes React bail
          // out of the render entirely, which is roughly one render per
          // twenty-six pixels instead of one per event.
          setScrollTop((prev) =>
            Math.floor(top / ROW_HEIGHT) === Math.floor(prev / ROW_HEIGHT) ? prev : top,
          );
          // The remembered offset is the real one, not the quantised one.
          // A position inside a search result set means nothing next launch:
          // the query is not remembered, so the offset would land somewhere
          // arbitrary in the category list instead.
          if (!searching) saveSession({ channelScrollTop: top });
        }}
      >
        <div className="spacer" style={{ height: win.totalHeight }}>
          {rows}
        </div>
        {sticky ? <div className="sticky-head">{sticky.cleanName}</div> : null}
        {source.total === 0 && !source.loading && !source.error ? (
          <div className="pane-note">
            {searching
              ? `Nothing matches “${state.query}”.`
              : state.categories.status === "error"
                ? // Empty because there is nothing here, or because the
                  // catalogue could not be read? Different things.
                  `Channels unavailable — ${state.categories.message}`
                : "No channels here."}
          </div>
        ) : null}
      </div>
    </main>
  );
}

/** What the list is showing, and the way to the guide. */
function ListHead({ title, total, loading }: { title: string; total: number; loading: boolean }) {
  return (
    <div className="list-head">
      <span className="list-title" title={title}>
        {title}
      </span>
      <span className="list-count">{total.toLocaleString()}</span>
      {loading ? <span className="list-count">loading…</span> : null}
      <span className="spring" />
      {/* Beside the list's own title: the guide is the same channels as a
          timeline, so the switch belongs to the list, not the window. */}
      <ViewSwitch current="browse" />
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
