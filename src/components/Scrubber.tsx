// The two-zone timeline: archive to the left of the seam, local buffer to the
// right. Left of the seam a seek reopens the stream from the server; right of
// it, it is instant. All the arithmetic is in logic/scrubber.ts.

import { useEffect, useRef, useState } from "react";
import { backend } from "../backend";
import { clock, hhmm } from "../logic/format";
import {
  behindLiveAt,
  catchupRequest,
  geometry,
  headPixel,
  seekCost,
  timeAtPixel,
} from "../logic/scrubber";
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
  const [width, setWidth] = useState(0);
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

  const pixelOf = (e: { clientX: number }) => {
    const el = trackRef.current;
    if (!el) return 0;
    return e.clientX - el.getBoundingClientRect().left;
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
          <div className="scrub-hint" style={{ left: hoverPx ?? 0 }}>
            {hhmm(hoverTime)} · {clock(Math.max(0, now - hoverTime))} back ·{" "}
            {hoverCost === "reconnect" ? "reconnects" : "instant"}
          </div>
        ) : null}
      </div>

      <div className="scrub-labels">
        {g.hasArchive ? (
          <>
            <span>{channel?.archiveDays}-day archive · seek reconnects</span>
            <span>
              buffer {clock(player.bufferedBack)} · instant
            </span>
          </>
        ) : (
          <>
            <span>
              local buffer only{channel ? " — no catchup on this channel" : ""}
            </span>
            <span>{clock(player.bufferedBack)} of scrollback</span>
          </>
        )}
      </div>
    </div>
  );
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
