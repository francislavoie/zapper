// The band across the top of browse mode: what is playing on the left, the
// picture pinned top-right beside it. The channel list scrolls underneath and
// never moves either of them.
//
// Two rules hold this file together.
//
//  1. The video slot is a hole, not a surface. mpv draws beneath the webview,
//     so the slot and everything inside it stay transparent — no background,
//     ever, not even while stopped.
//  2. Nothing is painted over the slot. The information sits strictly to its
//     left, and the list strictly below it.
//
// The geometry lives in two CSS custom properties, --video-w and --video-h, so
// moving or resizing the picture is one edit and the rect follows by
// measurement.

import { useRef, type PointerEvent as ReactPointerEvent, type RefObject } from "react";
import { playChannel } from "../actions";
import { slotAction } from "../logic/gestures";
import { backend } from "../backend";
import { useClock } from "../hooks/useClock";
import { useNowNext } from "../hooks/useNowNext";
import {
  hhmm,
  duration,
  liveOffset,
  measuredLine,
  playerBadge,
  programmeProgress,
} from "../logic/format";
import {
  VIDEO_WIDTH_DEFAULT,
  fittedVideoWidth,
  isDefaultVideoWidth,
  widthAfterDrag,
} from "../logic/videoSize";
import { run, useApp, useDispatch } from "../store";
import { Fullscreen, ResizeHandle, Stop } from "./icons";
import { MiniControls } from "./MiniControls";
import type { ChannelDto, ProgrammeDto } from "../types";

export interface WatchBandProps {
  /** The transparent hole mpv is told to draw into. */
  videoRef: RefObject<HTMLDivElement | null>;
}

export function WatchBand({ videoRef }: WatchBandProps) {
  const app = useApp();
  const { nowChannel, player } = app;
  // The width on screen, not the one remembered: a drag starts from what
  // the pointer is on, and the slot's pixels-per-rem is measured against it.
  const videoWidthRem = fittedVideoWidth(app.videoWidthRem, app.videoRoomRem);
  const dispatch = useDispatch();
  const now = useClock();
  const { now: onNow, next } = useNowNext(nowChannel, now);

  const live = player.state !== "stopped";
  const toWatch = () => dispatch({ type: "mode", mode: "watch" });
  // Restored from last time but deliberately not started. See App.tsx: a
  // provider may allow only one connection, and spending it at startup is a
  // guess about what this launch is for.
  const ready = !live && nowChannel !== null;
  const resume = () => {
    if (nowChannel) void playChannel(dispatch, nowChannel);
  };

  // The slot looks like the picture, so it has to behave like it: while
  // something is playing a click goes full screen, paused it resumes, and
  // while a channel is merely restored it starts it. Doing nothing was the
  // worst of the three — it said "ready" and then ignored the click. While
  // playing, the click arrives through MiniControls, which first waits to
  // see whether it is half of a double-click.
  const activate = () => {
    switch (slotAction(player.state, ready)) {
      case "resume":
        void run(dispatch, "pause", backend.togglePause());
        break;
      case "watch":
        toWatch();
        break;
      case "start":
        resume();
        break;
    }
  };

  // Resizing the picture. The slot is pinned right, so dragging its left
  // edge outwards makes it bigger.
  //
  // `pxPerRem` is measured from the slot rather than assumed to be 16: under
  // CSS zoom a rem and a client pixel are different sizes, and taking both
  // from the same box is what makes the drag track the pointer under either
  // zoom mechanism without knowing which is in use.
  const drag = useRef<{ x: number; rem: number; pxPerRem: number } | null>(null);

  const onGripDown = (e: ReactPointerEvent<HTMLElement>) => {
    const slot = videoRef.current;
    if (!slot) return;
    const rect = slot.getBoundingClientRect();
    if (!(rect.width > 0) || !(videoWidthRem > 0)) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { x: e.clientX, rem: videoWidthRem, pxPerRem: rect.width / videoWidthRem };
  };

  const onGripMove = (e: ReactPointerEvent<HTMLElement>) => {
    const d = drag.current;
    if (!d) return;
    dispatch({ type: "videoWidth", rem: widthAfterDrag(d.rem, e.clientX - d.x, d.pxPerRem) });
  };

  const endGrip = (e: ReactPointerEvent<HTMLElement>) => {
    if (!drag.current) return;
    drag.current = null;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
  };

  // What clicking the slot does, as its accessible name: a role="button" with
  // no text inside has nothing else for a screen reader to call it. Not shown
  // as a hover tip — a box of text floating over the picture is in the way
  // of the one thing this slot is for, and clicking it is self-explanatory.
  const videoTip =
    player.state === "paused"
      ? "Click to resume"
      : live
        ? "Back to full screen"
        : ready
          ? "Click to start this channel"
          : undefined;

  return (
    <section className="watch-band" aria-label="Now watching">
      <div className="watch-info">
        {live && nowChannel ? (
          <Playing
            channel={nowChannel}
            onNow={onNow}
            next={next}
            now={now}
            onFullScreen={toWatch}
            onStop={() => {
              void run(dispatch, "stop", backend.stop());
            }}
          />
        ) : ready && nowChannel ? (
          <Ready
            channel={nowChannel}
            onNow={onNow}
            onPlay={() => void playChannel(dispatch, nowChannel)}
          />
        ) : (
          <Idle />
        )}
      </div>

      {/* Beside the picture, not over it. Inside the slot it would be a
          control drawn on top of the video — and the video is
          the one surface in this app that nothing may be drawn on, because
          mpv is underneath and anything here is a hole punched in it.

          A separator rather than a button: it sets a value along an axis,
          which is what a separator with aria-valuenow is for. */}
      <span
        className="watch-grip"
        role="separator"
        aria-label="Picture size"
        aria-orientation="vertical"
        aria-valuenow={Math.round(videoWidthRem * 4) / 4}
        tabIndex={0}
        data-tip={
          isDefaultVideoWidth(videoWidthRem)
            ? "Drag to resize the picture"
            : "Drag to resize · double-click to reset"
        }
        onPointerDown={onGripDown}
        onPointerMove={onGripMove}
        onPointerUp={endGrip}
        onPointerCancel={endGrip}
        onDoubleClick={() => dispatch({ type: "videoWidth", rem: VIDEO_WIDTH_DEFAULT, reset: true })}
        onKeyDown={(e) => {
          // The same axis as the drag: left is bigger, because the slot
          // grows leftwards from a fixed right edge.
          const step = e.shiftKey ? 4 : 1;
          if (e.key === "ArrowLeft") {
            e.preventDefault();
            dispatch({ type: "videoWidth", rem: videoWidthRem + step });
          } else if (e.key === "ArrowRight") {
            e.preventDefault();
            dispatch({ type: "videoWidth", rem: videoWidthRem - step });
          } else if (e.key === "Home") {
            e.preventDefault();
            dispatch({ type: "videoWidth", rem: VIDEO_WIDTH_DEFAULT, reset: true });
          }
        }}
      >
        <ResizeHandle className="i" />
      </span>

      {/* Transparent by construction. Clickable all the same — a transparent
          element still takes clicks, and clicking the picture is how you get
          back to it full screen. */}
      <div
        className={`watch-video ${live ? "live" : ""} ${ready ? "ready" : ""}`}
        ref={videoRef}
        role={live || ready ? "button" : undefined}
        tabIndex={live || ready ? 0 : undefined}
        aria-label={videoTip}
        onClick={activate}
        onKeyDown={(e) => {
          if (e.key !== "Enter") return;
          if (live) toWatch();
          else if (ready) resume();
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          dispatch({ type: "videoMenu", at: { x: e.clientX, y: e.clientY } });
        }}
      >
        {live ? null : (
          <span className="watch-video-empty">
            {ready ? "click to resume" : "no picture"}
          </span>
        )}

        {/* Only while something is playing: there is nothing to transport
            otherwise, and the restored-channel slot is one big click target
            that these would fight with. */}
        {live ? <MiniControls onActivate={activate} /> : null}
      </div>
    </section>
  );
}

/** Nothing playing. The band explains what it is for rather than leaving an
 *  empty frame and letting the user guess. */
function Idle() {
  return (
    <div className="watch-idle">
      <h2>Nothing playing</h2>
      <p>
        Pick a channel and the picture appears top right. It keeps playing while
        you browse — nothing stops until you say so.
      </p>
      <ul className="watch-hints">
        <li>
          <b>/</b> search channels
        </li>
        <li>
          <b>G</b> the guide
        </li>
        <li>
          <b>Ctrl+P</b> everything else
        </li>
      </ul>
    </div>
  );
}

/** Where you left off: the channel is restored, the picture is not.
 *
 *  It says "Resume" rather than starting by itself on purpose. Opening a
 *  stream may cost the only connection the provider allows, and an app that starts
 *  playing the moment it is launched is an app that makes noise when you
 *  opened it to look something up. */
function Ready({
  channel,
  onNow,
  onPlay,
}: {
  channel: ChannelDto;
  onNow: ProgrammeDto | null;
  onPlay: () => void;
}) {
  return (
    <div className="watch-col identity">
      <p className="watch-state">Where you left off</p>
      <h2 data-tip-overflow={channel.name}>{channel.cleanName}</h2>
      <div className="watch-sub">
        {channel.num != null ? <span className="watch-num">#{channel.num}</span> : null}
        {onNow ? <span className="watch-now" data-tip-overflow="">{onNow.title}</span> : null}
        {channel.hasArchive ? (
          <span className="badge archive" data-tip={`${channel.archiveDays ?? "?"}-day catchup`}>
            ⏱ {channel.archiveDays ?? "?"}d catchup
          </span>
        ) : null}
      </div>
      <div className="watch-actions">
        <button className="ctl" onClick={onPlay}>
          Resume
        </button>
      </div>
    </div>
  );
}

function Playing({
  channel,
  onNow,
  next,
  now,
  onFullScreen,
  onStop,
}: {
  channel: ChannelDto;
  onNow: ProgrammeDto | null;
  next: ProgrammeDto | null;
  now: number;
  onFullScreen: () => void;
  onStop: () => void;
}) {
  const { player } = useApp();
  const measured = measuredLine(player);
  const badge = playerBadge(player);

  return (
    <>
      <div className="watch-col identity">
        {/* The name and, at the right of the same line, what you can do
            with it. Icons rather than worded buttons: two words-in-boxes
            under the name took a row of their own for two actions everybody
            recognises by shape. The name gives way first when it is long. */}
        <div className="watch-head">
          <h2 data-tip-overflow={channel.name}>{channel.cleanName}</h2>
          <div className="watch-head-actions">
            <button
              className="ctl icon"
              aria-label="Full screen"
              data-tip="Full screen (F)"
              onClick={onFullScreen}
            >
              <Fullscreen className="i" />
            </button>
            <button className="ctl icon" aria-label="Stop" data-tip="Stop" onClick={onStop}>
              <Stop className="i" />
            </button>
          </div>
        </div>
        <div className="watch-sub">
          {channel.num != null ? <span className="watch-num">#{channel.num}</span> : null}
          {badge ? <span className="badge">{badge}</span> : null}
          {channel.hasArchive ? (
            <span className="badge archive" data-tip={`${channel.archiveDays ?? "?"}-day catchup`}>
              ⏱ {channel.archiveDays ?? "?"}d catchup
            </span>
          ) : null}
        </div>
        {player.state === "opening" ? (
          <p className="watch-state">Opening…</p>
        ) : player.state === "failed" ? (
          <p className="watch-state bad">{player.error ?? "Stream failed"}</p>
        ) : null}
      </div>

      <div className="watch-col epg">
        {onNow ? (
          <div className="watch-prog">
            <div className="watch-prog-head">
              <span className="watch-when">NOW</span>
              <span className="watch-time">
                {hhmm(onNow.start)}–{hhmm(onNow.stop)}
              </span>
            </div>
            <div className="watch-title" data-tip-overflow="">{onNow.title}</div>
            <div className="watch-progress">
              <span
                style={{ width: `${programmeProgress(onNow.start, onNow.stop, now) * 100}%` }}
              />
            </div>
            {onNow.description ? <p className="watch-desc" data-tip-overflow="">{onNow.description}</p> : null}
          </div>
        ) : (
          // 15% EPG coverage: this is the common case, and it is not an error.
          <div className="watch-prog none">no guide data for this channel</div>
        )}

        {next ? (
          <div className="watch-prog">
            <div className="watch-prog-head">
              <span className="watch-when">NEXT</span>
              <span className="watch-time">{hhmm(next.start)}</span>
            </div>
            <div className="watch-title" data-tip-overflow="">{next.title}</div>
          </div>
        ) : null}
      </div>

      <dl className="watch-col facts">
        <div>
          <dt>Measured</dt>
          <dd>{measured || "not measured yet"}</dd>
        </div>
        <div>
          <dt>Position</dt>
          <dd>{liveOffset(player.atLiveEdge, player.behindLive)}</dd>
        </div>
        <div>
          <dt>Rewind buffer</dt>
          <dd>{duration(player.bufferedBack)}</dd>
        </div>
      </dl>
    </>
  );
}
