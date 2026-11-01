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
// Shift+wheel moves that offset by exactly the pixels the gesture reported.
//
// What is drawn eases toward it over a fifth of a second (useEasedTime).
// Vertical lists step instantly — see logic/wheel.ts — but a wheel notch here
// is a quarter of an hour of programmes sliding sideways, and moved at once
// that read as the guide jumping in fifteen-minute steps. The easing is in
// absolute time and chases a target that every notch moves on, so a fast
// spin still covers its full distance and ends exactly where it should.
//
// The pixel arithmetic is all in logic/guide.ts and checked there.

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { playCatchupProgramme, playChannel } from "../actions";
import { useChannelPages } from "../hooks/useChannelPages";
import { useClock } from "../hooks/useClock";
import { useEasedTime } from "../hooks/useEasedTime";
import { useGuideRows } from "../hooks/useGuideRows";
import { timeRange, timeTick } from "../logic/format";
import {
  GUIDE_FUTURE_HOURS,
  GUIDE_PAST_HOURS,
  GUIDE_ROW_HEIGHT,
  HOUR,
  anchorFor,
  clampHours,
  dayLabel,
  dragOffset,
  guideWindow,
  horizontalPixels,
  nowLineX,
  pageDirection,
  playability,
  programmeSpan,
  programmeStatus,
  rebaseOffset,
  scrollThumb,
  scrollbarX,
  secondsForPixels,
  showsNow,
  ticks,
  withinArchive,
  type TimeWindow,
} from "../logic/guide";
import { centredScrollTop } from "../logic/programmes";
import { OVERSCAN, computeWindow } from "../logic/virtual";
import { ALL_CATEGORIES, useApp, useDispatch } from "../store";
import type { ChannelDto, ProgrammeDto } from "../types";
import { RailExpand } from "./Rail";
import { ViewSwitch } from "./ViewSwitch";

/** Anything narrower than this cannot hold a legible title. */
const LABEL_MIN_PX = 34;
const TIME_MIN_PX = 96;

/** `besideRail`: this guide shares the browse grid with the rail, so it
 *  offers the way back to a folded rail. The stacked player's guide has no
 *  rail beside it to bring back. */
export function GuideGrid({ besideRail = false }: { besideRail?: boolean }) {
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
  // Where the guide is going: what the store's offset names.
  const target = useMemo(() => guideWindow(now, hours, offset), [now, hours, offset]);

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
  /** The rows' vertical scrollbar, which the time scrollbar below them stops
   *  short of so the two tracks end at the same x. */
  const [barInset, setBarInset] = useState(0);

  // Both dimensions are measured, never assumed: the row count per screen and
  // the seconds-per-pixel of the axis both depend on them.
  useEffect(() => {
    const body = bodyRef.current;
    const track = trackRef.current;
    if (!body || !track) return;
    const read = () => {
      setHeight(body.clientHeight);
      setTrackWidth(track.clientWidth);
      setBarInset(Math.max(0, body.offsetWidth - body.clientWidth));
    };
    read();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(read);
    ro.observe(body);
    ro.observe(track);
    return () => ro.disconnect();
  }, []);

  // Where the guide has got to: the target's left edge, eased. Arrived means
  // within a quarter of a pixel, or a second if that is more — closer than
  // that nothing on screen moves, and the last frames would be re-rendering
  // for nothing. While the scrollbar's thumb is held the drawn window is the
  // target exactly, so the thumb stays under the pointer.
  const [scrubbing, setScrubbing] = useState(false);
  const span = hours * HOUR;
  const snap = Math.max(1, secondsForPixels(0.25, hours, trackWidth));
  const shownFrom = useEasedTime(target.from, snap, scrubbing);
  const win = useMemo(() => ({ from: shownFrom, to: shownFrom + span }), [shownFrom, span]);

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

  const { ensure } = source;

  // Sent here to a channel, by choosing an upcoming programme in search:
  // scroll its row to the middle. After the reset above, which runs on the
  // same category change. It waits for a measured height, and for the row
  // to exist — a category's length is only a floor until its pages load, so
  // a row past the estimate has its page asked for first. Applied once per
  // request, so scrolling away afterwards is not fought.
  const focus = state.guideFocus;
  const appliedFocus = useRef<number | null>(null);
  useEffect(() => {
    if (!focus || focus.index === null || appliedFocus.current === focus.seq) return;
    const body = bodyRef.current;
    if (!body || height <= 0) return;
    if (source.total <= focus.index) {
      ensure(focus.index, focus.index + 1);
      return;
    }
    appliedFocus.current = focus.seq;
    const top = centredScrollTop(focus.index, GUIDE_ROW_HEIGHT, height);
    body.scrollTo({ top });
    setScrollTop(top);
  }, [focus, height, source.total, ensure]);

  const rows = computeWindow(scrollTop, height, GUIDE_ROW_HEIGHT, source.total, OVERSCAN);

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
    target,
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
      {/* Three groups, the middle one centred on the bar itself rather than
          on whatever room the sides leave — see .guide-head. */}
      <header className="guide-head">
        <div className="guide-head-start">
          {besideRail && state.railCollapsed ? <RailExpand /> : null}
          <div className="guide-head-title">
            <h2 data-tip-overflow="">{categoryName}</h2>
            <span className="guide-window">
              {day ? `${day} ` : ""}
              {timeRange(win.from, win.to)} · {hours}h
            </span>
          </div>
        </div>
        <div className="guide-nav">
          <button
            className="ctl"
            aria-label="Back 1 hour"
            data-tip="Back 1 hour (←)"
            onClick={() => dispatch({ type: "guideShift", hours: -1 })}
          >
            ‹
          </button>
          {/* Lit by where the guide is going, not where the animation has
              got to, so it does not flicker on the way past. */}
          <button
            className={`ctl ${showsNow(now, target) ? "on" : ""}`}
            data-tip="Back to now (Home)"
            onClick={() => dispatch({ type: "guideNow" })}
          >
            Now
          </button>
          <button
            className="ctl"
            aria-label="Forward 1 hour"
            data-tip="Forward 1 hour (→)"
            onClick={() => dispatch({ type: "guideShift", hours: 1 })}
          >
            ›
          </button>
        </div>
        <div className="guide-head-end">
          <ViewSwitch current="guide" />
        </div>
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
                  {timeTick(t)}
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
                focused={focus?.streamId === channel.streamId}
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

      <TimeScrollbar
        offset={win.from - anchor}
        anchor={anchor}
        hours={hours}
        now={now}
        inset={barInset}
        onScrub={setScrubbing}
        onSeek={(o) => dispatch({ type: "guideSeek", offset: o, anchor })}
        onPage={(dir) => dispatch({ type: "guideShift", hours: dir * hours })}
      />
    </div>
  );
}

/** The guide's horizontal scrollbar. The page has no wide strip for a native
 *  one to scroll — the guide moves through time by an offset — so this is
 *  drawn: the whole reach, a day back to two ahead, with a thumb for the
 *  window on screen and a mark where now is. Drag the thumb to scrub; click
 *  either side of it to move a screen that way. The arrow keys are the global
 *  handler's, which already move the guide, so this takes no focus of its
 *  own. Geometry in logic/guide.ts. */
function TimeScrollbar({
  offset,
  anchor,
  hours,
  now,
  inset,
  onScrub,
  onSeek,
  onPage,
}: {
  /** The drawn window's offset from `anchor`, eased: the thumb follows the
   *  animation rather than leaping ahead of it. */
  offset: number;
  anchor: number;
  hours: number;
  now: number;
  inset: number;
  onScrub: (on: boolean) => void;
  onSeek: (offset: number) => void;
  onPage: (dir: -1 | 1) => void;
}) {
  const trackRef = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const el = trackRef.current;
    if (!el) return;
    const read = () => setWidth(el.clientWidth);
    read();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(read);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  // The inset arrives a render after the first measurement and narrows the
  // track; read it again before paint rather than waiting on the observer,
  // so the first thumb drawn is already the right one.
  useLayoutEffect(() => {
    if (trackRef.current) setWidth(trackRef.current.clientWidth);
  }, [inset]);

  const thumb = scrollThumb(offset, hours, width);
  const nowX = scrollbarX(now, anchor, hours, width);
  const drag = useRef<{ x: number; offset: number } | null>(null);
  const [dragging, setDragging] = useState(false);

  const onThumbDown = (e: ReactPointerEvent<HTMLElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    // From where the thumb is drawn, so grabbing it mid-glide does not make
    // it leap to where the glide was going: the glide's target becomes
    // where it had got to, and the drag goes on from there.
    drag.current = { x: e.clientX, offset };
    onSeek(offset);
    setDragging(true);
    onScrub(true);
  };
  const onThumbMove = (e: ReactPointerEvent<HTMLElement>) => {
    const d = drag.current;
    if (!d) return;
    onSeek(dragOffset(d.offset, e.clientX - d.x, hours, width));
  };
  const endThumb = (e: ReactPointerEvent<HTMLElement>) => {
    if (!drag.current) return;
    drag.current = null;
    setDragging(false);
    onScrub(false);
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
  };
  const onTrackDown = (e: ReactPointerEvent<HTMLElement>) => {
    if (e.button !== 0) return;
    const rect = e.currentTarget.getBoundingClientRect();
    const dir = pageDirection(e.clientX - rect.left, thumb);
    if (dir !== 0) onPage(dir);
  };

  return (
    <div className="guide-scroll" style={{ paddingRight: inset }}>
      <div className="guide-scroll-corner" aria-hidden />
      <div
        className={`guide-scroll-track ${dragging ? "dragging" : ""}`}
        ref={trackRef}
        role="scrollbar"
        aria-label="Time"
        aria-orientation="horizontal"
        aria-valuemin={-GUIDE_PAST_HOURS}
        aria-valuemax={GUIDE_FUTURE_HOURS - hours}
        aria-valuenow={Math.round((offset / HOUR) * 4) / 4}
        data-tip="Drag to move through time · click either side to jump a screen"
        onPointerDown={onTrackDown}
      >
        <span
          className="guide-scroll-thumb"
          style={{ left: thumb.left, width: thumb.width }}
          onPointerDown={onThumbDown}
          onPointerMove={onThumbMove}
          onPointerUp={endThumb}
          onPointerCancel={endThumb}
        />
        {nowX >= 0 && nowX <= width ? (
          <span className="guide-scroll-now" style={{ left: nowX }} aria-hidden />
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
  focused,
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
  /** The channel search sent the guide to. */
  focused: boolean;
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
    <div className={`guide-row ${selected ? "sel" : ""} ${focused ? "focus" : ""}`} style={style}>
      <div className="guide-chan">
        <span className="guide-num">{channel.numLabel ?? channel.num ?? ""}</span>
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

  const time = timeRange(programme.start, programme.stop);
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
