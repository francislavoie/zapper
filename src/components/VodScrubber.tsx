// A movie's seek bar. The channel's Scrubber is a window onto a buffer behind
// a live edge; a movie has a start, an end and a position between them, so
// this is the plain kind: what has been read ahead, what has been played, and
// a knob.
//
// A click or a drag seeks, once, on release — a seek is a request to the
// provider's stream, and a drag that sent one per pixel would be a hundred.
// While dragging, the knob follows the pointer and the hint says where the
// release will land.
//
// The hint carries a preview of that moment where there is one: a frame
// drawn while the title played, or (in the app) one decoded from what has
// been buffered ahead or skipped over. Past what has been played or
// buffered there is none, and the hint is the time alone. Same card as a
// channel's scrubber, above the bar, or below it inside a small docked
// picture.
//
// On the focused bar ←/→ move by VOD_SKIP_SECONDS, the same as the global
// keys do; the event stops here so the one press is not two seeks.

import {
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
  type RefObject,
} from "react";
import { seekBy, seekTo } from "../actions";
import { useTitleThumbnail } from "../hooks/useTitleThumbnail";
import { clock } from "../logic/format";
import { hintFitsAbove, hintPixel } from "../logic/scrubber";
import {
  VOD_SKIP_SECONDS,
  bufferedFraction,
  playedFraction,
  seekTarget,
  vodTimes,
} from "../logic/vodTransport";
import { useDispatch } from "../store";
import type { OnDemandDto } from "../types";

export interface VodScrubberProps {
  onDemand: OnDemandDto;
  /** The docked picture's size. */
  compact?: boolean;
  /** The bar fades on idle. A pointer over the bar holds it open. */
  onPin: (pinned: boolean) => void;
  /** A box the hover preview must not rise out of, as for Scrubber. */
  hintWithin?: RefObject<HTMLElement | null>;
}

export function VodScrubber({ onDemand, compact = false, onPin, hintWithin }: VodScrubberProps) {
  const dispatch = useDispatch();
  const trackRef = useRef<HTMLDivElement | null>(null);
  const hintRef = useRef<HTMLDivElement | null>(null);
  // Where the pointer is over the bar, as a fraction; null away from it.
  const [hover, setHover] = useState<number | null>(null);
  // The fraction being dragged to; null when not dragging.
  const [drag, setDrag] = useState<number | null>(null);
  // The hint's and the bar's widths, in layout pixels, to keep the card
  // inside the bar; and whether it has to open below the bar.
  const [hintWidth, setHintWidth] = useState(0);
  const [barWidth, setBarWidth] = useState(0);
  const [hintBelow, setHintBelow] = useState(false);

  const { duration, position } = onDemand;
  const known = duration !== null && duration > 0;

  /** The fraction of the bar's width under the pointer: a ratio of two
   *  measurements in the same pixels, so CSS zoom cancels out. */
  const fractionAt = (e: { clientX: number }): number => {
    const el = trackRef.current;
    if (!el) return 0;
    const box = el.getBoundingClientRect();
    return box.width > 0 ? Math.min(1, Math.max(0, (e.clientX - box.left) / box.width)) : 0;
  };

  const seek = (seconds: number | null) => {
    if (seconds === null) return;
    void seekTo(dispatch, seconds);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!known) return;
    const by = e.key === "ArrowLeft" ? -VOD_SKIP_SECONDS : e.key === "ArrowRight" ? VOD_SKIP_SECONDS : 0;
    if (by === 0) return;
    e.preventDefault();
    e.stopPropagation();
    void seekBy(dispatch, by);
  };

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (!known || e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    setDrag(fractionAt(e));
  };

  const onPointerUp = (e: PointerEvent<HTMLDivElement>) => {
    if (drag === null) return;
    seek(seekTarget(fractionAt(e), duration));
    setDrag(null);
  };

  const shown = drag ?? hover;
  const played = drag ?? playedFraction(onDemand);
  const target = shown === null || !known ? null : seekTarget(shown, duration);
  const hint = target === null ? null : vodTimes(target, null).elapsed;
  const preview = useTitleThumbnail(`${onDemand.kind}:${onDemand.id}`, target, duration);

  // Measured before paint, as Scrubber does: the card is a picture wide
  // with one and a time wide without, and is never seen unclamped first.
  useLayoutEffect(() => {
    const el = hintRef.current;
    const bar = el?.parentElement;
    setHintWidth(el ? el.offsetWidth : 0);
    setBarWidth(bar ? bar.offsetWidth : 0);
    const bound = hintWithin?.current;
    if (!el || !bar || !bound) {
      setHintBelow(false);
      return;
    }
    setHintBelow(
      !hintFitsAbove(
        bar.getBoundingClientRect().top,
        el.getBoundingClientRect().height,
        bound.getBoundingClientRect().top,
      ),
    );
  }, [shown, preview, hintWithin]);

  return (
    <div className="scrub">
      <div
        className="vod-scrubber"
        role="slider"
        tabIndex={0}
        aria-label="Position"
        aria-valuemin={0}
        aria-valuemax={known ? duration : 0}
        aria-valuenow={position}
        aria-valuetext={clock(position)}
        data-compact={compact ? "" : undefined}
        onKeyDown={onKeyDown}
        onPointerEnter={() => onPin(true)}
        onPointerLeave={() => {
          onPin(false);
          setHover(null);
        }}
        onPointerMove={(e) => {
          const f = fractionAt(e);
          setHover(f);
          if (drag !== null) setDrag(f);
        }}
        onPointerDown={onPointerDown}
        onPointerUp={onPointerUp}
        onPointerCancel={() => {
          setDrag(null);
          setHover(null);
        }}
      >
        <div className="vod-track" ref={trackRef}>
          <div className="vod-buffered" style={{ width: `${bufferedFraction(onDemand) * 100}%` }} />
          <div className="vod-played" style={{ width: `${played * 100}%` }} />
        </div>
        <div className="vod-knob" style={{ left: `${played * 100}%` }} />
        {hint !== null && shown !== null ? (
          // Centred on the pointer and kept inside the bar at both ends.
          <div
            className={hintBelow ? "scrub-hint below" : "scrub-hint"}
            ref={hintRef}
            style={{ left: hintPixel(shown * barWidth, hintWidth, barWidth) }}
          >
            {/* The frame of the ten seconds or so around the pointer, not
                exactly the moment under it; the time is the pointer's own,
                which is where a release goes. */}
            {preview ? (
              <img className="scrub-thumb" src={preview.dataUrl} alt="" draggable={false} />
            ) : null}
            <span>{hint}</span>
          </div>
        ) : null}
      </div>
    </div>
  );
}
