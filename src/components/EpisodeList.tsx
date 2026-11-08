// A season's episodes in the Shows tab's episode pane (ShowPane): number,
// still, title, a line of plot, length and a ✓ to mark one watched, the
// next-up and the playing one tagged. A listbox, one focus stop: ↑/↓ (and
// Page, Home, End) move inside it, Enter or a double-click plays, as does a
// click on a still's ▶; the pane takes the keys that leave it (`onKeys`).
//
// Windowed: a season can run to hundreds of episodes, so only the rows in
// sight and a few either side are in the page. The list does not scroll by
// itself — the box around it does — so what is in sight is worked out from
// that scroller: how far it has scrolled past the list's top, and how tall
// it is. A sticky bar over its top edge is its `scroll-padding-top`, which
// the keys keep the chosen row clear of.

import {
  memo,
  useCallback,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
} from "react";
import { movieCursorKey, scrollToShow } from "../logic/movieKeys";
import { clock } from "../logic/format";
import { runtime } from "../logic/movies";
import { timeLeft } from "../logic/resume";
import { EPISODE_ROW_HEIGHT, episodeProgress, episodeTitle, episodeWindow } from "../logic/shows";
import type { EpisodeDto } from "../types";
import { Poster } from "./Poster";

export interface EpisodeListProps {
  episodes: readonly EpisodeDto[];
  /** The episode the show's main action plays, tagged Next. */
  nextUp: number | null;
  /** The episode the player has open, tagged Playing. */
  playingId?: number | null;
  onPlay: (e: EpisodeDto) => void;
  onWatched: (e: EpisodeDto, watched: boolean) => void;
  /** Keys the list leaves to whatever holds it (the pane's `[`, `]`, ← and
   *  Escape): `true` when one was taken, and the list does no more with it. */
  onKeys?: (key: string) => boolean;
  /** Scroll the next-up episode (or the list's top) into sight when the
   *  list appears. Off for a list whose scroller should start at its top. */
  revealOnMount?: boolean;
}

/** The nearest ancestor that scrolls vertically. */
function scrollerOf(el: HTMLElement): HTMLElement | null {
  for (let p = el.parentElement; p; p = p.parentElement) {
    const y = getComputedStyle(p).overflowY;
    if (y === "auto" || y === "scroll") return p;
  }
  return null;
}

/** Where `el` starts inside `scroller`'s content, in the same units as its
 *  scrollTop (layout pixels, whatever the zoom). */
function offsetWithin(el: HTMLElement, scroller: HTMLElement): number {
  let top = 0;
  let at: HTMLElement | null = el;
  while (at && at !== scroller) {
    top += at.offsetTop;
    const parent = at.offsetParent as HTMLElement | null;
    // An offset parent outside the scroller: count from the scroller's own.
    if (parent && !scroller.contains(parent)) return top - scroller.offsetTop;
    at = parent;
  }
  return top;
}

interface Geometry {
  scroller: HTMLElement;
  /** The list's top in the scroller's content. */
  offset: number;
  /** What a sticky bar covers of the scroller's top. */
  inset: number;
}

const H = EPISODE_ROW_HEIGHT;

export function EpisodeList({
  episodes,
  nextUp,
  playingId = null,
  onPlay,
  onWatched,
  onKeys,
  revealOnMount = true,
}: EpisodeListProps) {
  const listRef = useRef<HTMLDivElement | null>(null);
  const count = episodes.length;
  const nextIndex = episodes.findIndex((e) => e.episodeId === nextUp);
  // The row the keys move from: the next-up one to begin with, else the
  // first, so Enter on arriving always has one to play.
  const [cursor, setCursor] = useState(nextIndex >= 0 ? nextIndex : 0);
  const [win, setWin] = useState(() => episodeWindow(0, 0, count));

  const geometry = useCallback((): Geometry | null => {
    const el = listRef.current;
    const scroller = el ? scrollerOf(el) : null;
    if (!el || !scroller) return null;
    const inset = parseFloat(getComputedStyle(scroller).scrollPaddingTop) || 0;
    return { scroller, offset: offsetWithin(el, scroller), inset };
  }, []);

  const measure = useCallback(() => {
    const g = geometry();
    if (!g) return;
    const next = episodeWindow(Math.max(0, g.scroller.scrollTop - g.offset), g.scroller.clientHeight, count);
    setWin((w) => (w.start === next.start && w.end === next.end && w.totalHeight === next.totalHeight ? w : next));
  }, [geometry, count]);

  // Brings row `i` into sight below the sticky bar, moving as little as it can.
  const reveal = useCallback(
    (i: number) => {
      const g = geometry();
      if (!g) return;
      const sc = g.scroller;
      const top = scrollToShow(g.offset + i * H, H, sc.scrollTop + g.inset, sc.clientHeight - g.inset) - g.inset;
      if (top !== sc.scrollTop) sc.scrollTo({ top: Math.max(0, top) });
    },
    [geometry],
  );

  // A season shown: what is next in sight if it is in this one, else its
  // first episode, wherever the last one was left. Once per season: the
  // pane keys the list by it. The pane's own box scrolls, so nothing else
  // on the page moves with it.
  useLayoutEffect(() => {
    if (!revealOnMount) return;
    const g = geometry();
    if (!g) return;
    const sc = g.scroller;
    if (nextIndex >= 0) reveal(nextIndex);
    else if (sc.scrollTop > g.offset - g.inset) sc.scrollTo({ top: Math.max(0, g.offset - g.inset) });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useLayoutEffect(() => {
    const sc = geometry()?.scroller;
    if (!sc) return;
    measure();
    sc.addEventListener("scroll", measure, { passive: true });
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    ro?.observe(sc);
    return () => {
      sc.removeEventListener("scroll", measure);
      ro?.disconnect();
    };
  }, [geometry, measure]);

  // The rows' handlers, the same functions from render to render so their
  // memoised renders hold.
  const handlers = useRef({ onPlay, onWatched });
  handlers.current = { onPlay, onWatched };
  const play = useCallback((e: EpisodeDto) => handlers.current.onPlay(e), []);
  const mark = useCallback((e: EpisodeDto, w: boolean) => handlers.current.onWatched(e, w), []);
  const choose = useCallback((i: number) => setCursor(i), []);

  // A refreshed season can be shorter than the cursor was.
  const at = cursor < count ? cursor : count - 1;

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    // The list's own keys only: Enter on a row's ✓ is that button's, and
    // must mark that episode rather than play the row the cursor is on.
    if (e.target !== e.currentTarget) return;
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (onKeys?.(e.key)) {
      // Nor the player's keys, nor Escape's chain, as well.
      e.preventDefault();
      e.stopPropagation();
      return;
    }
    if (e.key === "Enter") {
      const ep = at >= 0 ? episodes[at] : undefined;
      if (ep) {
        e.preventDefault();
        onPlay(ep);
      }
      return;
    }
    const g = geometry();
    const to = movieCursorKey(e.key, {
      current: at,
      total: count,
      columns: 1,
      visibleRows: g ? Math.max(1, Math.floor((g.scroller.clientHeight - g.inset) / H)) : 1,
      posters: false,
    });
    if (to === null) return;
    // Neither the player's keys nor the sheet's own scrolling as well.
    e.preventDefault();
    e.stopPropagation();
    setCursor(to);
    reveal(to);
  };

  const idPrefix = `ep${useId()}`;
  const rows = [];
  for (let i = win.start; i < Math.min(win.end, count); i++) {
    const ep = episodes[i];
    rows.push(
      <EpisodeRow
        key={ep.episodeId}
        episode={ep}
        index={i}
        id={`${idPrefix}-${ep.episodeId}`}
        next={ep.episodeId === nextUp}
        playing={ep.episodeId === playingId}
        selected={i === at}
        onChoose={choose}
        onPlay={play}
        onWatched={mark}
      />,
    );
  }
  const shown = at >= win.start && at < win.end ? episodes[at] : undefined;

  return (
    <div
      className="ep-list"
      role="listbox"
      aria-label="Episodes"
      tabIndex={0}
      ref={listRef}
      aria-activedescendant={shown ? `${idPrefix}-${shown.episodeId}` : undefined}
      style={{ height: win.totalHeight }}
      onKeyDown={onKeyDown}
    >
      {rows}
    </div>
  );
}

const stop = (e: MouseEvent) => e.stopPropagation();

const EpisodeRow = memo(function EpisodeRow({
  episode: e,
  index,
  id,
  next,
  playing,
  selected,
  onChoose,
  onPlay,
  onWatched,
}: {
  episode: EpisodeDto;
  index: number;
  id: string;
  next: boolean;
  playing: boolean;
  selected: boolean;
  onChoose: (i: number) => void;
  onPlay: (e: EpisodeDto) => void;
  onWatched: (e: EpisodeDto, watched: boolean) => void;
}) {
  const title = episodeTitle(e);
  const number = e.episodeEnd !== null && e.episodeEnd > e.episode ? `${e.episode}–${e.episodeEnd}` : String(e.episode);
  const progress = episodeProgress(e);
  const length = runtime(e.durationSecs ?? e.duration);
  // How far in, before the plot: "12:40 of 44:00".
  const where =
    progress !== null && e.position !== null
      ? `${clock(e.position)} of ${clock(e.duration ?? e.durationSecs ?? 0)}`
      : "";
  const line = [where, e.plot ?? ""].filter(Boolean).join(" · ");
  return (
    <div
      className={`ep ${next ? "next" : ""} ${playing ? "playing" : ""} ${selected ? "sel" : ""}`}
      id={id}
      role="option"
      aria-selected={selected}
      style={{ top: index * H, height: H }}
      onClick={() => onChoose(index)}
      onDoubleClick={() => onPlay(e)}
    >
      <span className="ep-num">{number}</span>
      <span
        className="ep-still"
        onClick={(ev) => {
          ev.stopPropagation();
          onPlay(e);
        }}
        onDoubleClick={stop}
      >
        <Poster url={e.stillUrl} name={title} className="ep-still-img" />
        {progress !== null ? (
          <span className="ep-bar">
            <span style={{ width: `${Math.round(progress * 100)}%` }} />
          </span>
        ) : null}
        {e.watched ? (
          <span className="ep-seen" aria-hidden>
            ✓
          </span>
        ) : null}
        <span className="ep-play" aria-hidden>
          ▶
        </span>
      </span>
      <span className="ep-text">
        <span className="ep-head">
          <span className="ep-title" data-tip-overflow="">
            {title}
          </span>
          {playing ? (
            <span className="ep-tag">Playing</span>
          ) : next ? (
            <span className="ep-tag">Next</span>
          ) : null}
        </span>
        {line ? (
          <span className="ep-plot" data-tip-overflow="">
            {line}
          </span>
        ) : null}
      </span>
      <span className="ep-len">
        {length}
        {progress !== null ? <span className="ep-left">{timeLeft({ ...e })}</span> : null}
      </span>
      <button
        className={`ctl icon ep-check ${e.watched ? "on" : ""}`}
        aria-label={e.watched ? "Mark unwatched" : "Mark watched"}
        aria-pressed={e.watched}
        data-tip={e.watched ? "Mark unwatched" : "Mark watched"}
        onClick={(ev) => {
          ev.stopPropagation();
          onWatched(e, !e.watched);
        }}
        onDoubleClick={stop}
      >
        ✓
      </button>
    </div>
  );
});
