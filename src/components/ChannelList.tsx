// The virtualized list. Tens of thousands of rows, ~47 DOM nodes.
//
// Rows are absolutely positioned inside a spacer of the full height, so the
// scrollbar is honest about how much list there is while only the visible
// window exists. Windowing arithmetic lives in logic/virtual.ts and is
// checked there; this file is the wiring.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { forgetChannel, playChannel } from "../actions";
import { backend } from "../backend";
import { registerZapList, type ZapList } from "../mediaKeys";
import { registerSearchInput } from "../focus";
import type { RowSource } from "../hooks/useChannelPages";
import { useClock } from "../hooks/useClock";
import { useGuideRows } from "../hooks/useGuideRows";
import { useLiveRows } from "../hooks/useLiveRows";
import { useProgrammeSearch } from "../hooks/useProgrammeSearch";
import { useSearch } from "../hooks/useSearch";
import { HOUR, rowNowNext } from "../logic/guide";
import { favouriteMeta } from "../logic/favourites";
import { ALL_CATEGORIES, pagedCategory, railItemKind, selectionSubtitle, selectionTitle } from "../logic/railItems";
import { centredScrollTop } from "../logic/programmes";
import { LIST_ROW_HEIGHT, OVERSCAN, computeWindow, stickyIndex } from "../logic/virtual";
import { loadSession, saveSession } from "../session";
import { useApp, useDispatch } from "../store";
import type { Async } from "../logic/async";
import type { SearchTab } from "../logic/programmes";
import { SEARCH_PLACEHOLDER } from "../logic/searchQuery";
import { categoryNames, groupByCategory } from "../logic/searchGroups";
import type { ChannelDto } from "../types";
import { ChannelRow } from "./ChannelRow";
import { FavouriteMenu } from "./FavouriteMenu";
import { ViewSwitch } from "./ViewSwitch";
import { ProgrammeResults } from "./ProgrammeResults";
import { SearchField } from "./SearchField";
import { RailToggle } from "./Rail";

/** The backend caps search at a sane number; 100 is plenty to scan. */
const SEARCH_LIMIT = 100;
/** Programme results per page; "More results" fetches the next. */
const PROGRAMME_PAGE = 100;

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
  const heading = selectionTitle(
    state.categoryId,
    state.categories.status === "ready" ? state.categories.data : [],
  );

  // Recordings are not channels, and the list is hidden behind them: it keeps
  // the last channel selection, so the media keys still have something to
  // step through.
  const channelSelection = useRef(ALL_CATEGORIES);
  if (railItemKind(state.categoryId) !== "recordings") channelSelection.current = state.categoryId;
  const rowsId = railItemKind(state.categoryId) === "recordings" ? channelSelection.current : state.categoryId;
  const live = useLiveRows(rowsId);
  const paged = pagedCategory(rowsId);
  // `/` and Ctrl+P run through the same backend index; the difference is only
  // where the results are shown.
  const results = useSearch(state.query, SEARCH_LIMIT);
  const names = useMemo(
    () => categoryNames(state.categories.status === "ready" ? state.categories.data : []),
    [state.categories],
  );
  // Under their categories, since a name alone does not say where a channel
  // is; see logic/searchGroups.
  const grouped = useMemo(() => asSource(results, names), [results, names]);
  const source: RowSource = searching ? grouped : live;
  /** Channels, not rows: a search's headings are not results. */
  const channelCount =
    searching && results.status === "ready" ? results.data.length : source.total;
  // Both kinds are searched together, so each tab can say how much it has
  // before it is chosen. Which one is showing outlives the query: someone
  // looking for programmes types one search after another.
  const programmes = useProgrammeSearch(state.query, PROGRAMME_PAGE);
  const [tab, setTab] = useState<SearchTab>("channels");
  const showingProgrammes = searching && tab === "programmes";
  const programmeCount =
    programmes.result.status === "ready" ? programmes.result.data.length : 0;

  // What the media keys' next and previous step through. Updated on every
  // render, hidden ones included: behind the player this list still follows
  // the category a favourite was played from, but its effects do not run.
  const zap = useRef<ZapList | null>(null);
  zap.current = {
    rowAt: source.rowAt,
    total: source.total,
    // Only a selection the backend pages can be fetched into: favourites and
    // recent are whole lists already, and recordings are not channels.
    fetch:
      searching || paged === undefined
        ? undefined
        : (start, count) => backend.channels(paged, start, count),
  };
  useEffect(() => registerZapList(() => zap.current), []);

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
    const top = Math.min(want, Math.max(0, source.total * LIST_ROW_HEIGHT - height));
    restoreTo.current = 0;
    el.scrollTo({ top });
    setScrollTop(top);
  }, [height, source.total]);

  const win = computeWindow(scrollTop, height, LIST_ROW_HEIGHT, source.total, OVERSCAN);

  // Sent here to a channel by the band's "Show its category": scroll its row
  // to the middle. After the reset above, which runs on the same category
  // change. It waits for a measured height, and for the row to exist — a
  // category's length is only a floor until its pages load, so a row past
  // the estimate has its page asked for first. Applied once per request, so
  // scrolling away afterwards is not fought.
  const focus = state.guideFocus;
  const appliedFocus = useRef<number | null>(null);

  const { ensure } = source;
  useEffect(() => {
    if (win.end > win.start) ensure(win.start, win.end);
  }, [ensure, win.start, win.end]);

  useEffect(() => {
    if (!focus || focus.index === null || appliedFocus.current === focus.seq) return;
    const el = viewportRef.current;
    if (!el || height <= 0) return;
    if (source.total <= focus.index) {
      ensure(focus.index, focus.index + 1);
      return;
    }
    appliedFocus.current = focus.seq;
    const top = centredScrollTop(focus.index, LIST_ROW_HEIGHT, height);
    restoreTo.current = 0;
    el.scrollTo({ top });
    setScrollTop(top);
  }, [focus, height, source.total, ensure]);

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

  const firstVisible = Math.floor(scrollTop / LIST_ROW_HEIGHT);
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

  // What is on, and next, for the rows in view only: the same guide data the
  // grid reads, asked for the window around now and re-read each minute.
  const minute = useClock(60_000);
  const visibleIds: number[] = [];
  for (let i = win.start; i < win.end; i++) {
    const c = source.rowAt(i);
    if (c && !c.isSeparator) visibleIds.push(c.streamId);
  }
  const guide = useGuideRows(
    visibleIds,
    { from: minute - HOUR, to: minute + 4 * HOUR },
    state.sync.lastSynced,
  );

  // Where each channel lives, as the second line under its name. Left off
  // where the list is already one category's, or grouped under headings.
  const showCategory = !searching && (live.kind === "all" || live.listed !== null);

  const listedEntries = live.kind === "favourites" ? state.favourites : live.kind === "recent" ? state.recent : null;
  const entries = listedEntries && listedEntries.status === "ready" ? listedEntries.data : null;
  // Read through a ref, like the playing state above, so the row callback
  // keeps its identity while the lists reload underneath it.
  const menuSource = useRef({ entries, minute });
  menuSource.current = { entries, minute };
  const onMenu = useCallback(
    (c: ChannelDto, x: number, y: number) => {
      const { entries: list, minute: at } = menuSource.current;
      const entry = list?.find((f) => f.channel.streamId === c.streamId);
      if (!entry) return;
      dispatch({
        type: "favouriteMenu",
        menu: {
          streamId: c.streamId,
          name: c.cleanName,
          meta: favouriteMeta(entry.watchSeconds, entry.lastWatched, at),
          x,
          y,
        },
      });
    },
    [dispatch],
  );

  const rows = [];
  for (let i = win.start; i < win.end; i++) {
    const c = source.rowAt(i);
    const top = i * LIST_ROW_HEIGHT;
    if (!c) {
      rows.push(<div className="row" key={`gap-${i}`} style={{ top, height: LIST_ROW_HEIGHT }} />);
      continue;
    }
    // What playing it has since added to its record, over what the page
    // was fetched with. A new object only for the few rows that have news,
    // so the rest keep their memoised render.
    const news = state.qualityNews;
    const quality = c.streamId in news ? news[c.streamId] : c.quality;
    const nowNext = rowNowNext(c, guide.programmesFor(c.streamId), minute);
    const shown = quality !== c.quality ? { ...c, quality } : c;
    rows.push(
      <ChannelRow
        key={c.streamId}
        channel={shown}
        categoryName={showCategory && c.categoryId ? (names.get(c.categoryId) ?? null) : null}
        now={nowNext.now}
        next={nowNext.next}
        minute={minute}
        onMenu={listedEntries ? onMenu : undefined}
        top={top}
        selected={state.player.streamId === c.streamId}
        focused={focus?.streamId === c.streamId}
        pending={state.pendingStreamId === c.streamId}
        onPlay={onPlay}
        onOpenFull={onOpenFull}
        clock={state.settings.timeFormat}
      />,
    );
  }

  return (
    <main className="list-pane">
      {/* One heading row, level with the rail's filter row beside it. */}
      <div className="list-head">
        <RailToggle />
        <span className="list-title" data-tip-overflow="">
          {searching ? "Search results" : heading}
        </span>
        <ListCount
          text={
            showingProgrammes
              ? `${programmeCount.toLocaleString()}${programmes.hasMore ? "+" : ""}`
              : !searching && listedEntries
                ? selectionSubtitle(rowsId, channelCount)
                : `${channelCount.toLocaleString()}`
          }
          loading={showingProgrammes ? programmes.result.status === "loading" : source.loading}
        />
        <SearchField
          className="list-search"
          inputRef={registerSearchInput}
          placeholder={SEARCH_PLACEHOLDER}
          value={state.query}
          onChange={(text) => dispatch({ type: "query", text })}
          clearLabel="Clear search"
        />
        <span className="spring" />
        {searching ? (
          <SearchKind
            tab={tab}
            onTab={setTab}
            channels={channelCount}
            programmes={programmeCount}
            moreProgrammes={programmes.hasMore}
          />
        ) : null}
        {/* Beside the list's own title: the guide is the same channels as a
            timeline, so the switch belongs to the list, not the window. */}
        <ViewSwitch current="browse" />
      </div>

      {showingProgrammes ? null : (
        <div className="list-cols" aria-hidden>
          <span>#</span>
          <span />
          <span>Channel</span>
          <span>On now</span>
          <span>Next</span>
        </div>
      )}

      {showingProgrammes ? <ProgrammeResults search={programmes} query={state.query} /> : null}

      {source.error && !showingProgrammes ? (
        <div className="pane-error">
          <span>Channels unavailable — {source.error}</span>
          <button className="link" onClick={source.retry}>
            try again
          </button>
        </div>
      ) : null}

      <div
        className="viewport"
        // Hidden rather than unmounted while programmes show: its size
        // observer is attached once, on mount, and would not come back.
        hidden={showingProgrammes}
        ref={viewportRef}
        onScroll={(e) => {
          const top = e.currentTarget.scrollTop;
          // Quantised to the row grid, and this is the difference between a
          // list that scrolls and one that drags. Nothing rendered here
          // depends on where inside a row the viewport sits: the rows are
          // absolutely positioned at index * LIST_ROW_HEIGHT, so only *which*
          // rows exist changes. Storing the raw offset re-rendered the whole
          // window on every scroll event — dozens a second, each one
          // reconciling forty-odd rows to produce identical output. Keeping
          // the previous value when the row has not changed makes React bail
          // out of the render entirely, which is roughly one render per
          // twenty-six pixels instead of one per event.
          setScrollTop((prev) =>
            Math.floor(top / LIST_ROW_HEIGHT) === Math.floor(prev / LIST_ROW_HEIGHT) ? prev : top,
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
        {source.total === 0 && !source.loading && !source.error && (searching || live.kind !== "recordings") ? (
          <div className="pane-note">
            {searching
              ? `Nothing matches “${state.query}”.`
              : live.kind === "favourites"
                ? // Not an error: this is what the first run looks like, and
                  // there is nothing for anyone to do about it.
                  "Nothing here yet. Favourites collect themselves from what you watch — a channel joins the list after about half a minute on it."
                : live.kind === "recent"
                  ? "Nothing here yet. Channels you watch for half a minute or more appear here, newest first."
                  : state.categories.status === "error"
                ? // Empty because there is nothing here, or because the
                  // catalogue could not be read? Different things.
                  `Channels unavailable — ${state.categories.message}`
                : "No channels here."}
          </div>
        ) : null}
      </div>
      {state.favouriteMenu ? (
        <FavouriteMenu
          menu={state.favouriteMenu}
          onForget={() => void forgetChannel(dispatch, state.favouriteMenu!.streamId)}
          onClose={() => dispatch({ type: "favouriteMenu", menu: null })}
        />
      ) : null}
    </main>
  );
}

/** How much the list holds, and that more is on its way. */
function ListCount({ text, loading }: { text: string; loading: boolean }) {
  return (
    <>
      <span className="list-count">{text}</span>
      {loading ? <span className="list-count">loading…</span> : null}
    </>
  );
}

/** Channels | Programmes, beside the search box while there is a search. A
 *  tab rather than one mixed list, because the two answer different
 *  questions — "where is this channel" and "when is this on" — and each
 *  ranks its own way. */
function SearchKind({
  tab,
  onTab,
  channels,
  programmes,
  moreProgrammes,
}: {
  tab: SearchTab;
  onTab: (tab: SearchTab) => void;
  channels: number;
  programmes: number;
  moreProgrammes: boolean;
}) {
  const kinds: { id: SearchTab; label: string; count: string }[] = [
    { id: "channels", label: "Channels", count: channels.toLocaleString() },
    {
      id: "programmes",
      label: "Programmes",
      count: `${programmes.toLocaleString()}${moreProgrammes ? "+" : ""}`,
    },
  ];
  return (
    <div className="view-switch search-kind" role="tablist" aria-label="Search for">
      {kinds.map((k) => (
        <button
          key={k.id}
          role="tab"
          aria-selected={tab === k.id}
          className={tab === k.id ? "on" : ""}
          // Keeps focus in the search box, so typing carries on after a switch.
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => onTab(k.id)}
        >
          {k.label} <span className="search-kind-n">{k.count}</span>
        </button>
      ))}
    </div>
  );
}

/** Search results are a plain array — no paging, the backend caps the count —
 *  regrouped under a heading per category. */
function asSource(r: Async<ChannelDto[]>, names: ReadonlyMap<string, string>): RowSource {
  const data = r.status === "ready" ? groupByCategory(r.data, names) : [];
  return {
    total: data.length,
    rowAt: (i) => data[i],
    ensure: NOOP_ENSURE,
    error: r.status === "error" ? r.message : null,
    loading: r.status === "loading",
    retry: NOOP,
  };
}
