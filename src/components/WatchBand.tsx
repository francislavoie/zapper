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

import type { RefObject } from "react";
import { backend } from "../backend";
import { useClock } from "../hooks/useClock";
import { useNowNext } from "../hooks/useNowNext";
import {
  clock,
  hhmm,
  liveOffset,
  measuredLine,
  playerBadge,
  programmeProgress,
} from "../logic/format";
import { run, useApp, useDispatch } from "../store";
import type { ChannelDto, ProgrammeDto } from "../types";

export interface WatchBandProps {
  /** The transparent hole mpv is told to draw into. */
  videoRef: RefObject<HTMLDivElement | null>;
}

export function WatchBand({ videoRef }: WatchBandProps) {
  const { nowChannel, player } = useApp();
  const dispatch = useDispatch();
  const now = useClock();
  const { now: onNow, next } = useNowNext(nowChannel, now);

  const live = player.state !== "stopped";
  const toWatch = () => dispatch({ type: "mode", mode: "watch" });

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
        ) : (
          <Idle />
        )}
      </div>

      {/* Transparent by construction. Clickable all the same — a transparent
          element still takes clicks, and clicking the picture is how you get
          back to it full screen. */}
      <div
        className={`watch-video ${live ? "live" : ""}`}
        ref={videoRef}
        role={live ? "button" : undefined}
        tabIndex={live ? 0 : undefined}
        title={live ? "Back to full screen" : undefined}
        onClick={() => live && toWatch()}
        onKeyDown={(e) => {
          if (e.key === "Enter" && live) toWatch();
        }}
      >
        {live ? null : <span className="watch-video-empty">no picture</span>}
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
        <h2 title={channel.name}>{channel.cleanName}</h2>
        <div className="watch-sub">
          {channel.num != null ? <span className="watch-num">#{channel.num}</span> : null}
          {badge ? <span className="badge">{badge}</span> : null}
          {channel.hasArchive ? (
            <span className="badge archive" title={`${channel.archiveDays ?? "?"}-day catchup`}>
              ⏱ {channel.archiveDays ?? "?"}d catchup
            </span>
          ) : null}
        </div>
        {player.state === "opening" ? (
          <p className="watch-state">Opening…</p>
        ) : player.state === "failed" ? (
          <p className="watch-state bad">{player.error ?? "Stream failed"}</p>
        ) : null}
        <div className="watch-actions">
          <button className="ctl" onClick={onFullScreen}>
            Full screen
          </button>
          <button className="ctl ghost" onClick={onStop}>
            Stop
          </button>
        </div>
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
            <div className="watch-title">{onNow.title}</div>
            <div className="watch-progress">
              <span
                style={{ width: `${programmeProgress(onNow.start, onNow.stop, now) * 100}%` }}
              />
            </div>
            {onNow.description ? <p className="watch-desc">{onNow.description}</p> : null}
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
            <div className="watch-title">{next.title}</div>
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
          <dd>{liveOffset(player.behindLive)}</dd>
        </div>
        <div>
          <dt>Rewind buffer</dt>
          <dd>{clock(player.bufferedBack)}</dd>
        </div>
      </dl>
    </>
  );
}
