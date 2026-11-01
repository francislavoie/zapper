// The two-zone timeline: archive to the left of the seam, local buffer to the
// right. Left of the seam a seek reopens the stream from the server; right of
// it, it is instant. All the arithmetic is in logic/scrubber.ts.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { backend } from "../backend";
import { duration, hhmm } from "../logic/format";
import {
  behindLiveAt,
  catchupRequest,
  geometry,
  headPixel,
  hintPixel,
  localPixel,
  seekCost,
  timeAtPixel,
} from "../logic/scrubber";
import { useThumbnail } from "../hooks/useThumbnail";
import { run, useDispatch } from "../store";
import type { ChannelDto, PlayerStateDto } from "../types";

export interface ScrubberProps {
  player: PlayerStateDto;
  channel: ChannelDto | null;
  onPin: (pinned: boolean) => void;
}

export function Scrubber({ player, channel, onPin }: ScrubberProps) {
  const dispatch = useDispatch();
  const trackRef = useRef<HTMLDivElement | null>(null);
  const hintRef = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(0);
  const [hintWidth, setHintWidth] = useState(0);
  const [hoverPx, setHoverPx] = useState<number | null>(null);
  const now = useNowSeconds();

  useEffect(() => {
    const el = trackRef.current;
    if (!el) return;
    setWidth(el.clientWidth);
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const g = geometry({
    width,
    hasArchive: channel?.hasArchive ?? false,
    archiveDays: channel?.archiveDays ?? null,
    bufferedBack: player.bufferedBack,
    now,
  });

  const headPx = headPixel(g, player);
  const hoverTime = hoverPx === null ? null : timeAtPixel(g, hoverPx);
  const hoverCost = hoverTime === null ? null : seekCost(g, hoverTime);

  // Only the buffer has previews. Left of the seam is the server's archive,
  // which has never been on this screen and so was never photographed; the
  // hint there is the time alone, which is what it always was.
  const preview = useThumbnail(
    hoverTime !== null && hoverCost === "instant" ? behindLiveAt(g, hoverTime) : null,
  );

  // Measured rather than estimated: the text is a time, a duration and a
  // word, and it changes width as the pointer moves. Before paint, so the
  // tooltip is never seen at the unclamped position first.
  useLayoutEffect(() => {
    const el = hintRef.current;
    setHintWidth(el ? el.getBoundingClientRect().width / scaleOf(el) : 0);
  }, [hoverTime, hoverCost, width]);

  // Layout pixels, which is what the geometry is in. See `localPixel`: the
  // rect and the event are in visual pixels and `clientWidth` is not, and
  // under CSS zoom those are different numbers.
  const pixelOf = (e: { clientX: number }) => {
    const el = trackRef.current;
    if (!el) return 0;
    const box = el.getBoundingClientRect();
    return localPixel(e.clientX, box.left, box.width, el.clientWidth);
  };

  const seekTo = (target: number) => {
    if (!channel) return;
    if (seekCost(g, target) === "reconnect") {
      // Left of the seam: a new connection to the server's timeshift URL.
      const { start, durationMinutes } = catchupRequest(g, target);
      void run(
        dispatch,
        "catchup",
        backend.playCatchup(channel.streamId, start, durationMinutes),
      );
      return;
    }
    const seconds = player.behindLive - behindLiveAt(g, target);
    void run(dispatch, "seek", backend.seekRelative(seconds));
  };

  const bufferIsEmpty = player.bufferedBack < 1;

  return (
    <div className="scrub">
      <div
        className="scrubber"
        onPointerEnter={() => onPin(true)}
        onPointerLeave={() => {
          onPin(false);
          setHoverPx(null);
        }}
        onPointerMove={(e) => setHoverPx(pixelOf(e))}
        onClick={(e) => seekTo(timeAtPixel(g, pixelOf(e)))}
      >
        <div className="track" ref={trackRef}>
          {g.hasArchive ? (
            <div className="zone archive" style={{ left: 0, width: g.seamPx }} />
          ) : null}
          <div
            className={`zone ${bufferIsEmpty ? "unbuffered" : "buffer"}`}
            style={{ left: g.seamPx, right: 0 }}
          />
          {g.hasArchive ? <div className="seam" style={{ left: g.seamPx }} /> : null}
          <div className="playhead" style={{ left: headPx }} />
        </div>

        {hoverTime !== null ? (
          <div
            className="scrub-hint"
            ref={hintRef}
            style={{ left: hintPixel(hoverPx ?? 0, hintWidth, width) }}
          >
            {/* The nearest captured frame, which is within a couple of
                seconds of the cursor rather than exactly under it — the
                backend samples, it does not keep every frame. The caption
                stays the cursor's own time, because that is where a click
                actually goes. */}
            {preview ? (
              <img className="scrub-thumb" src={preview.dataUrl} alt="" draggable={false} />
            ) : null}
            <span>
              {hhmm(hoverTime)} · {duration(Math.max(0, now - hoverTime))} back ·{" "}
              {hoverCost === "reconnect" ? "reconnects" : "instant"}
            </span>
          </div>
        ) : null}
      </div>

      <div className="scrub-labels">
        {g.hasArchive ? (
          <>
            <span>{channel?.archiveDays}-day archive · seek reconnects</span>
            <span>
              buffer {duration(player.bufferedBack)} · instant
            </span>
          </>
        ) : (
          <>
            <span>
              local buffer only{channel ? " — no catchup on this channel" : ""}
            </span>
            <span>{duration(player.bufferedBack)} of scrollback</span>
          </>
        )}
      </div>
    </div>
  );
}

/** How many visual pixels this element draws per layout pixel. One under
 *  page zoom, the zoom factor under CSS zoom, and the thing that has to be
 *  divided out before a measured width can be compared with `clientWidth`. */
function scaleOf(el: HTMLElement): number {
  const box = el.getBoundingClientRect();
  return el.offsetWidth > 0 && box.width > 0 ? box.width / el.offsetWidth : 1;
}

/** Unix seconds, ticking. The live edge moves whether or not React does. */
function useNowSeconds(): number {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const t = window.setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1000);
    return () => window.clearInterval(t);
  }, []);
  return now;
}
