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

import {
  useMemo,
  useRef,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { playChannel, showChannelCategory, showChannelInGuide } from "../actions";
import { endedMessage, startingLine } from "../logic/ending";
import { chordLabel } from "../logic/keys";
import { IS_MAC } from "../platform";
import { slotAction } from "../logic/gestures";
import { backend } from "../backend";
import { useClock } from "../hooks/useClock";
import { useNowNext } from "../hooks/useNowNext";
import {
  hhmm,
  timeRange,
  duration,
  liveOffset,
  measuredLine,
  offAirLabel,
  playerBadge,
  programmeProgress,
} from "../logic/format";
import {
  VIDEO_WIDTH_DEFAULT,
  fittedVideoWidth,
  isDefaultVideoWidth,
  widthAfterDrag,
} from "../logic/videoSize";
import { useRecordingChannels } from "../hooks/useRecordingChannels";
import { listedFor, railItemKind } from "../logic/railItems";
import { categoryNames } from "../logic/searchGroups";
import { RecordingBand } from "./RecordingBand";
import { moviesAvailable, playingKind } from "../logic/movies";
import { showsAvailable } from "../logic/shows";
import { run, useApp, useDispatch } from "../store";
import { MovieBand, PlayingEnd } from "./MovieBand";
import { ShowBand } from "./ShowBand";
import { Fullscreen, ResizeHandle, Stop } from "./icons";
import { MiniControls } from "./MiniControls";
import { StreamEnded } from "./StreamEnded";
import { Alternatives } from "./Alternatives";
import { usePlayerRecord } from "../hooks/usePlayerRecord";
import { PlayerRecordLine } from "./PlayerRecord";
import type { ChannelDto, ProgrammeDto } from "../types";
import type { UpNext } from "../logic/upNext";

export interface WatchBandProps {
  /** The transparent hole mpv is told to draw into. */
  videoRef: RefObject<HTMLDivElement | null>;
  /** Whether there is a picture to draw. Without one (a browser, for now) the
   *  band is only the information: no slot, no handle. */
  picture: boolean;
  /** Up next, for the docked picture's card (App's useUpNext). */
  upNext: UpNext;
}

export function WatchBand({ videoRef, picture, upNext }: WatchBandProps) {
  const app = useApp();
  const { nowChannel, player } = app;
  // The width on screen, not the one remembered: a drag starts from what
  // the pointer is on, and the slot's pixels-per-rem is measured against it.
  const videoWidthRem = fittedVideoWidth(app.videoWidthRem, app.videoRoomRem);
  const dispatch = useDispatch();
  const now = useClock();
  const { now: onNow, next } = useNowNext(nowChannel, now);

  // While Recordings is chosen the band is about the recording picked in
  // the list; with none picked it shows the channel as usual.
  const recording =
    railItemKind(app.categoryId) === "recordings" && app.selectedRecordingId !== null
      ? (app.recordings?.find((r) => r.id === app.selectedRecordingId) ?? null)
      : null;
  const recordingList = useMemo(() => (recording ? [recording] : null), [recording]);
  const recordingChannel = useRecordingChannels(recordingList).get(recording?.streamId ?? -1) ?? null;

  // Which body the band has, first match: the picked recording; in the
  // Movies tab the selected movie; in the Shows tab the selected show; a
  // movie that is playing; an episode that is playing (its show's band);
  // the channel.
  // Choosing a movie or a show to look at changes this and nothing else —
  // the picture beside it is whatever was already on.
  const inMovies = app.section === "movies" && moviesAvailable(app.movies.summary);
  const selectedMovie = inMovies ? app.movies.selected : null;
  const inShows = app.section === "shows" && showsAvailable(app.shows.summary);
  const selectedShow = inShows ? app.shows.selected : null;
  const movieOpen = playingKind(player) === "movie";
  const openMovieId = player.onDemand?.kind === "movie" ? player.onDemand.id : null;
  const episodeOpen = playingKind(player) === "episode";
  const playingEpisode = episodeOpen ? app.shows.playing : null;

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
    // The main button only: a side button's press is a step back, not a drag.
    if (e.button !== 0) return;
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
        {recording ? (
          <RecordingBand key={recording.id} rec={recording} channel={recordingChannel} />
        ) : selectedMovie ? (
          <MovieBand
            key={selectedMovie.movieId}
            movie={selectedMovie}
            playing={movieOpen && openMovieId === selectedMovie.movieId}
          />
        ) : selectedShow ? (
          <ShowBand
            key={selectedShow.seriesId}
            show={selectedShow}
            playing={playingEpisode?.series.seriesId === selectedShow.seriesId ? playingEpisode : null}
          />
        ) : movieOpen ? (
          app.movies.playing ? (
            <MovieBand key={app.movies.playing.movieId} movie={app.movies.playing} playing />
          ) : (
            // Open, but the title could not be looked up: still stoppable.
            <>
              <div className="watch-body">
                <h2 className="watch-title-h">Playing a movie</h2>
              </div>
              <div className="watch-actions">
                <PlayingEnd />
              </div>
            </>
          )
        ) : episodeOpen ? (
          playingEpisode ? (
            <ShowBand
              key={playingEpisode.series.seriesId}
              show={playingEpisode.series}
              playing={playingEpisode}
            />
          ) : (
            // Open, but not looked up yet: still stoppable.
            <>
              <div className="watch-body">
                <h2 className="watch-title-h">Playing an episode</h2>
              </div>
              <div className="watch-actions">
                <PlayingEnd />
              </div>
            </>
          )
        ) : live && nowChannel ? (
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
            next={next}
            now={now}
            onPlay={() => void playChannel(dispatch, nowChannel)}
          />
        ) : (
          <Idle picture={picture} movies={inMovies} shows={inShows} />
        )}
      </div>

      {picture ? (
        <>
      {/* Beside the picture, not over it. Inside the slot it would be a
          control drawn on top of the video — and the video is the one surface in this app that nothing may be drawn on, because
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
            that these would fight with. A stream that stopped by itself
            gets its explanation and Retry here instead; the details are in
            the column beside it. */}
        {player.state === "failed" || player.reconnect ? (
          <StreamEnded compact />
        ) : live ? (
          <MiniControls onActivate={activate} upNext={upNext} />
        ) : null}
      </div>
        </>
      ) : null}
    </section>
  );
}

/** Nothing playing. The band explains what it is for rather than leaving an
 *  empty frame and letting the user guess. */
function Idle({
  picture,
  movies = false,
  shows = false,
}: {
  picture: boolean;
  movies?: boolean;
  shows?: boolean;
}) {
  if (!picture) {
    return (
      <div className="watch-idle">
        <h2>Nothing selected</h2>
        <p>
          {movies
            ? "Choose a movie to see its details."
            : shows
              ? "Choose a show to see its details."
              : "Choose a recording to see what can be done with it."}
        </p>
      </div>
    );
  }
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
          <b>{chordLabel(["Ctrl", "P"], IS_MAC)}</b> everything else
        </li>
      </ul>
    </div>
  );
}

/** The programme first, as the mockup has it: its title, the channel's
 *  category, the channel with the programme's times, how far along it is, what
 *  it is about, and what follows. Without guide data the channel's name takes
 *  the title's place. Shared by the playing and the restored channel. */
function ChannelProgramme({
  channel,
  onNow,
  next,
  now,
  badge,
  notice,
}: {
  channel: ChannelDto;
  onNow: ProgrammeDto | null;
  next: ProgrammeDto | null;
  now: number;
  badge?: string;
  /** Said straight under the channel's line, before the programme's
   *  details: what must be in view without scrolling the column. */
  notice?: ReactNode;
}) {
  const { categories, qualityNews } = useApp();
  // What playing it has since added to its record, over what the channel
  // was fetched with: found off air a moment ago, say.
  const quality = channel.streamId in qualityNews ? qualityNews[channel.streamId] : channel.quality;
  const off = offAirLabel(quality, now);
  const category =
    channel.categoryId !== null && categories.status === "ready"
      ? categoryNames(categories.data).get(channel.categoryId)
      : undefined;
  return (
    <>
      <h2 className="watch-title-h" data-tip-overflow={onNow ? onNow.title : channel.name}>
        {onNow ? onNow.title : channel.cleanName}
      </h2>
      {category ? (
        <p className="watch-cat" data-tip-overflow="">
          {category}
        </p>
      ) : null}
      <div className="watch-sub">
        <span className="watch-chan" data-tip-overflow={channel.name}>
          {channel.cleanName}
        </span>
        {channel.num != null ? <span className="watch-num">#{channel.numLabel ?? channel.num}</span> : null}
        {badge ? <span className="badge">{badge}</span> : null}
        {channel.hasArchive ? (
          <span className="badge archive" data-tip={`${channel.archiveDays ?? "?"}-day catchup`}>
            ⏱ {channel.archiveDays ?? "?"}d catchup
          </span>
        ) : null}
        {onNow ? <span className="watch-time">{timeRange(onNow.start, onNow.stop)}</span> : null}
        {off ? (
          <span className="badge offair" data-tip="Your provider sent a placeholder instead of the channel">
            {off}
          </span>
        ) : null}
      </div>
      {notice}
      {onNow ? (
        <>
          <div className="watch-prog-line">
            <div className="watch-progress">
              <span style={{ width: `${programmeProgress(onNow.start, onNow.stop, now) * 100}%` }} />
            </div>
            <span className="watch-left">{duration(Math.max(0, onNow.stop - now))} left</span>
          </div>
          {onNow.description ? (
            <p className="watch-desc" data-tip-overflow="">
              {onNow.description}
            </p>
          ) : null}
        </>
      ) : (
        // 15% EPG coverage: this is the common case, and it is not an error.
        <p className="watch-none">no guide data for this channel</p>
      )}
      {next ? (
        <p className="watch-next" data-tip-overflow="">
          Next {hhmm(next.start)} {next.title}
        </p>
      ) : null}
    </>
  );
}

/** Show its category and Guide: where the channel is in the lists. Each is
 *  left out where it would go nowhere new. */
function ChannelLinks({ channel }: { channel: ChannelDto }) {
  const { categoryId, mode, favourites, recent } = useApp();
  const dispatch = useDispatch();
  return (
    <>
      {channel.categoryId !== null && channel.categoryId !== categoryId ? (
        <button className="ctl" onClick={() => void showChannelCategory(dispatch, channel)}>
          Show its category
        </button>
      ) : null}
      {mode !== "guide" ? (
        <button
          className="ctl"
          onClick={() =>
            void showChannelInGuide(dispatch, channel, {
              categoryId,
              listed: listedFor(
                categoryId,
                favourites.status === "ready" ? favourites.data : null,
                recent.status === "ready" ? recent.data : null,
              ),
            })
          }
        >
          Guide
        </button>
      ) : null}
    </>
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
  next,
  now,
  onPlay,
}: {
  channel: ChannelDto;
  onNow: ProgrammeDto | null;
  next: ProgrammeDto | null;
  now: number;
  onPlay: () => void;
}) {
  return (
    <>
      <div className="watch-body">
        <p className="watch-state">Where you left off</p>
        <ChannelProgramme channel={channel} onNow={onNow} next={next} now={now} />
      </div>
      <div className="watch-actions">
        <button className="ctl" onClick={onPlay}>
          Resume
        </button>
        <ChannelLinks channel={channel} />
      </div>
    </>
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
  const ended = endedMessage(player);
  const starting = startingLine(player);
  const record = usePlayerRecord();
  const facts = [
    measured,
    liveOffset(player.atLiveEdge, player.behindLive),
    `Rewind ${duration(player.bufferedBack)}`,
  ].filter(Boolean);

  const endedBlock = ended ? (
    <>
      <p className="watch-state bad">{ended.title}</p>
      {ended.detail ? <p className="watch-state">{ended.detail}</p> : null}
      {ended.code ? <p className="watch-state">{ended.code}</p> : null}
      {ended.alternatives && player.streamId !== null ? <Alternatives streamId={player.streamId} inline /> : null}
    </>
  ) : null;
  // Other sources to try are the point of the message, so it goes above the
  // programme's details, where the column shows it without scrolling.
  const upFront = !starting && ended?.alternatives === true;

  return (
    <>
      <div className="watch-body">
        <ChannelProgramme
          channel={channel}
          onNow={onNow}
          next={next}
          now={now}
          badge={playerBadge(player) ?? undefined}
          notice={upFront ? <div className="watch-notice">{endedBlock}</div> : undefined}
        />
        {upFront ? null : <p className="watch-facts">{facts.join(" · ")}</p>}
        {starting ? <p className="watch-state">{starting}</p> : upFront ? null : endedBlock}
      </div>
      <div className="watch-actions">
        {record ? <PlayerRecordLine record={record} /> : null}
        <ChannelLinks channel={channel} />
        {/* Icons at the end: shapes everybody recognises, out of the way of
            the actions that are about this programme. */}
        <span className="watch-actions-end">
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
        </span>
      </div>
    </>
  );
}
