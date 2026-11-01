// The guide. A time axis across the top, one row per channel in the current
// category, programmes placed by start and stop within the window.
//
// Three things make this survivable at tens of thousands of channels:
//
//  - rows are virtualized exactly like the channel list, so the grid is ~30
//    DOM rows however long the category is;
//  - `guide_rows` is asked only about the rows on screen, coalesced;
//  - a channel with no EPG — often most of them — gets one deliberate muted
//    band saying so, not an empty row that reads as a loading bug.
//
// Sideways, the guide scrolls through time rather than across a wide strip:
// the position is an offset in seconds in the store, and every render draws
// the window that offset names. A wheel, a tilt wheel, a touchpad swipe or
// Shift+wheel moves it by exactly the pixels the gesture reported, at once —
// no glide, for the same reasons logic/wheel.ts gives for vertical scrolling.
//
// The pixel arithmetic is all in logic/guide.ts and checked there.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { playCatchupProgramme, playChannel } from "../actions";
import { useChannelPages } from "../hooks/useChannelPages";
import { useClock } from "../hooks/useClock";
import { useGuideRows } from "../hooks/useGuideRows";
import { hhmm } from "../logic/format";
import {
  GUIDE_ROW_HEIGHT,
  anchorFor,
  clampHours,
  dayLabel,
  guideWindow,
  horizontalPixels,
  nowLineX,
  playability,
  programmeSpan,
  programmeStatus,
  rebaseOffset,
  secondsForPixels,
  showsNow,
  ticks,
  withinArchive,
  type TimeWindow,
} from "../logic/guide";
import { OVERSCAN, computeWindow } from "../logic/virtual";
import { ALL_CATEGORIES, useApp, useDispatch } from "../store";
import type { ChannelDto, ProgrammeDto } from "../types";
import { ViewSwitch } from "./ViewSwitch";

/** Anything narrower than this cannot hold a legible title. */
const LABEL_MIN_PX = 34;
const TIME_MIN_PX = 96;

export function GuideGrid() {
  const state = useApp();
  const dispatch = useDispatch();
  const now = useClock(15_000);

  const hours = clampHours(state.settings.guideHours);
  // The offset is relative to the half hour the clock is in. When the clock
  // moves on, the store is told so it can carry the offset across; until it
  // has been, this render carries it itself, so no frame is drawn half an
  // hour out.
  const anchor = anchorFor(now);
  const offset = rebaseOffset(state.guideOffset, state.guideAnchor, anchor, hours);
  useLayoutEffect(() => {
    if (state.guideAnchor !== anchor) dispatch({ type: "guideRebase", anchor });
  }, [anchor, state.guideAnchor, dispatch]);
  const win = useMemo(() => guideWindow(now, hours, offset), [now, hours, offset]);

  const categoryId = state.categoryId === ALL_CATEGORIES ? null : state.categoryId;
  const estimate =
    state.categories.status === "ready"
      ? categoryId === null
        ? state.categories.data.reduce((n, c) => n + c.channelCount, 0)
        : (state.categories.data.find((c) => c.id === categoryId)?.channelCount ?? 0)
      : state.sync.channelsTotal;
  const source = useChannelPages(categoryId, estimate);

  const bodyRef = useRef<HTMLDivElement | null>(null);
  const trackRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [height, setHeight] = useState(0);
  const [trackWidth, setTrackWidth] = useState(0);

  // Both dimensions are measured, never assumed: the row count per screen and
  // the seconds-per-pixel of the axis both depend on them.
  useEffect(() => {
    const body = bodyRef.current;
    const track = trackRef.current;
    if (!body || !track) return;
    const read = () => {
      setHeight(body.clientHeight);
      setTrackWidth(track.clientWidth);
    };
    read();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(read);
    ro.observe(body);
    ro.observe(track);
    return () => ro.disconnect();
  }, []);

  // Sideways wheel movement is the guide's own. A native listener because
  // React's onWheel is passive and could not stop the engine acting on the
  // event too. Vertical movement is not touched here: it bubbles on to the
  // page's wheel handling (useWheelSteps), which scrolls the rows exactly as
  // it scrolls every other list. The scale is read through a ref so the
  // listener is installed once rather than on every resize.
  const scale = useRef({ hours, trackWidth });
  scale.current = { hours, trackWidth };
  useEffect(() => {
    const body = bodyRef.current;
    if (!body) return;
    const onWheel = (e: WheelEvent) => {
      if (e.defaultPrevented) return;
      const { hours, trackWidth } = scale.current;
      const px = horizontalPixels(e, trackWidth);
      if (px === null) return;
      e.preventDefault();
      const seconds = secondsForPixels(px, hours, trackWidth);
      if (seconds !== 0) dispatch({ type: "guideScroll", seconds });
    };
    body.addEventListener("wheel", onWheel, { passive: false });
    return () => body.removeEventListener("wheel", onWheel);
  }, [dispatch]);

  // A different category is a different list: start at the top.
  useEffect(() => {
    bodyRef.current?.scrollTo({ top: 0 });
    setScrollTop(0);
  }, [state.categoryId]);

  const rows = computeWindow(scrollTop, height, GUIDE_ROW_HEIGHT, source.total, OVERSCAN);

  const { ensure } = source;
  useEffect(() => {
    if (rows.end > rows.start) ensure(rows.start, rows.end);
  }, [ensure, rows.start, rows.end]);

  const visible: ChannelDto[] = [];
  for (let i = rows.start; i < rows.end; i++) {
    const c = source.rowAt(i);
    if (c) visible.push(c);
  }
  const guide = useGuideRows(
    visible.filter((c) => !c.isSeparator).map((c) => c.streamId),
    win,
    state.sync.lastSynced,
  );

  const onPlay = useCallback(
    (channel: ChannelDto, p: ProgrammeDto) =>
      void playChannel(dispatch, { ...channel, nowPlaying: p }),
    [dispatch],
  );
  const onCatchup = useCallback(
    (channel: ChannelDto, p: ProgrammeDto) =>
      void playCatchupProgramme(dispatch, channel, p),
    [dispatch],
  );

  const nowX = nowLineX(now, win, trackWidth);
  const marks = ticks(win);
  const day = dayLabel(win.from, now);
  const categoryName =
    categoryId === null
      ? "All channels"
      : state.categories.status === "ready"
        ? (state.categories.data.find((c) => c.id === categoryId)?.name ?? "Guide")
        : "Guide";

  return (
    <div className="guide">
      <header className="guide-head">
        <h2>{categoryName}</h2>
        <span className="guide-window">
          {day ? `${day} ` : ""}
          {hhmm(win.from)}–{hhmm(win.to)} · {hours}h
        </span>
        <span className="spring" />
        <button
          className="ctl"
          data-tip="Back one screen (←)"
          onClick={() => dispatch({ type: "guideShift", hours: -hours })}
        >
          ‹
        </button>
        <button
          className={`ctl ${showsNow(now, win) ? "on" : ""}`}
          data-tip="Back to now (Home)"
          onClick={() => dispatch({ type: "guideNow" })}
        >
          Now
        </button>
        <button
          className="ctl"
          data-tip="Forward one screen (→)"
          onClick={() => dispatch({ type: "guideShift", hours })}
        >
          ›
        </button>
        <ViewSwitch current="guide" />
      </header>

      {source.error ? (
        <div className="pane-error">
          <span>Channels unavailable — {source.error}</span>
          <button className="link" onClick={source.retry}>
            try again
          </button>
        </div>
      ) : null}
      {guide.error ? (
        <div className="pane-error">
          <span>Guide data unavailable — {guide.error}</span>
          <button className="link" onClick={guide.retry}>
            try again
          </button>
        </div>
      ) : null}

      <div
        className="guide-body"
        ref={bodyRef}
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
      >
        {/* Sticky rather than a separate element: inside the scroller it is
            exactly as wide as the rows, so the axis and the programmes cannot
            disagree about where 20:30 is. */}
        <div className="guide-axis">
          <div className="guide-chan axis-corner">Channel</div>
          <div className="guide-track" ref={trackRef}>
            {marks.map((t) => {
              const left = ((t - win.from) / (win.to - win.from)) * trackWidth;
              return (
                <span key={t} className="guide-tick" style={{ left }}>
                  {hhmm(t)}
                </span>
              );
            })}
          </div>
        </div>

        <div className="guide-spacer" style={{ height: rows.totalHeight }}>
          {nowX === null ? null : (
            // Spans the whole spacer, so it scrolls with the rows and stays a
            // single continuous line rather than a per-row tick.
            <div
              className="guide-now"
              style={{ left: `calc(var(--guide-gutter) + ${nowX}px)` }}
              aria-hidden
            />
          )}

          {visible.map((channel, i) => {
            const index = rows.start + i;
            return (
              <GuideRow
                key={channel.streamId}
                channel={channel}
                top={index * GUIDE_ROW_HEIGHT}
                programmes={guide.programmesFor(channel.streamId)}
                win={win}
                width={trackWidth}
                now={now}
                selected={state.player.streamId === channel.streamId}
                onPlay={onPlay}
                onCatchup={onCatchup}
              />
            );
          })}
        </div>

        {source.total === 0 && !source.loading && !source.error ? (
          <div className="pane-note">
            {/* An empty grid because there is nothing here, or because the
                catalogue could not be read? Those are different, and saying
                the wrong one sends someone hunting for channels that were
                never fetched. */}
            {state.categories.status === "error"
              ? `Channels unavailable — ${state.categories.message}`
              : "No channels here."}
          </div>
        ) : null}
      </div>
    </div>
  );
}

function GuideRow({
  channel,
  top,
  programmes,
  win,
  width,
  now,
  selected,
  onPlay,
  onCatchup,
}: {
  channel: ChannelDto;
  top: number;
  programmes: ProgrammeDto[] | undefined;
  win: TimeWindow;
  width: number;
  now: number;
  selected: boolean;
  onPlay: (c: ChannelDto, p: ProgrammeDto) => void;
  onCatchup: (c: ChannelDto, p: ProgrammeDto) => void;
}) {
  const style = { top, height: GUIDE_ROW_HEIGHT };

  if (channel.isSeparator) {
    return (
      <div className="guide-row sep" style={style} role="separator">
        <div className="guide-chan">{channel.cleanName}</div>
      </div>
    );
  }

  return (
    <div className={`guide-row ${selected ? "sel" : ""}`} style={style}>
      <div className="guide-chan">
        <span className="guide-num">{channel.num ?? ""}</span>
        <span className="guide-name" data-tip-overflow={channel.name}>
          {channel.cleanName}
        </span>
        {channel.hasArchive ? <span className="guide-arch" data-tip="Has catch-up">⏱</span> : null}
      </div>
      <div className="guide-track">
        {programmes === undefined ? (
          <div className="prog loading" />
        ) : programmes.length === 0 ? (
          // The common case, and it must look deliberate.
          <div className="prog none">no guide data</div>
        ) : (
          // Keyed by start, which programmesFor() makes unique: an index key
          // would shift as blocks scroll off the left and remount every row.
          programmes.map((p) => (
            <Programme
              key={p.start}
              channel={channel}
              programme={p}
              win={win}
              width={width}
              now={now}
              onPlay={onPlay}
              onCatchup={onCatchup}
            />
          ))
        )}
      </div>
    </div>
  );
}

function Programme({
  channel,
  programme,
  win,
  width,
  now,
  onPlay,
  onCatchup,
}: {
  channel: ChannelDto;
  programme: ProgrammeDto;
  win: TimeWindow;
  width: number;
  now: number;
  onPlay: (c: ChannelDto, p: ProgrammeDto) => void;
  onCatchup: (c: ChannelDto, p: ProgrammeDto) => void;
}) {
  const span = programmeSpan(programme, win, width);
  // Outside the window, or a zero-length entry the guide data carried: nothing to
  // draw and nothing to click.
  if (!span || span.width <= 0) return null;

  const status = programmeStatus(programme, now);
  let act = playability(status, channel.hasArchive);
  if (act === "catchup" && !withinArchive(programme, now, channel.archiveDays)) {
    act = "inert";
  }

  const time = `${hhmm(programme.start)}–${hhmm(programme.stop)}`;
  const label = span.width >= LABEL_MIN_PX ? programme.title : "";
  const style = { left: span.left, width: span.width };
  const classes = [
    "prog",
    status,
    act === "inert" ? "inert" : "",
    span.clippedStart ? "clip-start" : "",
    span.clippedEnd ? "clip-end" : "",
  ]
    .filter(Boolean)
    .join(" ");

  if (act === "inert") {
    const why =
      status === "future"
        ? "Not on yet"
        : channel.hasArchive
          ? `Older than the ${channel.archiveDays ?? "?"}-day archive`
          : "Already finished, and this channel has no catchup";
    return (
      <div className={classes} style={style} data-tip={`${time}  ${programme.title} — ${why}`}>
        <span className="prog-title">{label}</span>
      </div>
    );
  }

  return (
    <button
      className={classes}
      style={style}
      data-tip={`${time}  ${programme.title} — ${act === "play" ? "on now: play" : "play from the archive"}`}
      onClick={() => (act === "play" ? onPlay(channel, programme) : onCatchup(channel, programme))}
    >
      <span className="prog-title">{label}</span>
      {span.width >= TIME_MIN_PX ? <span className="prog-time">{time}</span> : null}
    </button>
  );
}
