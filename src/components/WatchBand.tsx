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
  Fragment,
  useMemo,
  useRef,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { playChannel, removeFromContinue, showChannelCategory, showChannelInGuide } from "../actions";
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
import { continueShown } from "../logic/continueWatching";
import { categoryNames } from "../logic/searchGroups";
import { RecordingBand } from "./RecordingBand";
import { bandLayout, type BandItem } from "../logic/band";
import { moviesAvailable, playingKind } from "../logic/movies";
import { showsAvailable } from "../logic/shows";
import { run, useApp, useDispatch } from "../store";
import { MovieBand, MovieNowSide, MovieSelectedSide, PlayingEnd } from "./MovieBand";
import { EpisodeBand, EpisodeNowSide, ShowBand, ShowSelectedSide } from "./ShowBand";
import { BandLogo, BandMain, BandSide, Kicker, ProgressLine, SwapButton } from "./BandParts";
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
  const { now: onNow, next, later } = useNowNext(nowChannel, now);

  // While Recordings is chosen the band is about the recording picked in
  // the list; with none picked it shows the channel as usual.
  const recording =
    railItemKind(app.categoryId) === "recordings" && app.selectedRecordingId !== null
      ? (app.recordings?.find((r) => r.id === app.selectedRecordingId) ?? null)
      : null;
  const recordingList = useMemo(() => (recording ? [recording] : null), [recording]);
  const recordingChannel = useRecordingChannels(recordingList).get(recording?.streamId ?? -1) ?? null;

  // What the band tells of (logic/band.ts): what is selected in the Movies
  // or Shows tab, what is playing, or both side by side. Choosing a movie or
  // a show to look at changes this and nothing else — the picture beside it
  // is whatever was already on.
  const inMovies = app.section === "movies" && moviesAvailable(app.movies.summary);
  const inShows = app.section === "shows" && showsAvailable(app.shows.summary);
  // With Continue watching chosen and nothing typed, the band is about its
  // selected entry: the movie, or an episode's show.
  const tab = inMovies ? app.movies : inShows ? app.shows : null;
  const inContinue = tab !== null && continueShown(tab.selection, tab.query);
  const continueEntry = inContinue ? app.continue.selected : null;
  const selectedMovie = inContinue
    ? continueEntry?.kind === "movie"
      ? continueEntry.movie
      : null
    : inMovies
      ? app.movies.selected
      : null;
  const selectedShow = inContinue
    ? continueEntry?.kind === "episode"
      ? continueEntry.series
      : null
    : inShows
      ? app.shows.selected
      : null;
  const removeEntry = continueEntry ? (
    <button className="ctl watch-actions-end" onClick={() => void removeFromContinue(dispatch, continueEntry)}>
      Remove from Continue watching
    </button>
  ) : undefined;
  const movieOpen = playingKind(player) === "movie";
  const openMovieId = player.onDemand?.kind === "movie" ? player.onDemand.id : null;
  const episodeOpen = playingKind(player) === "episode";
  const playingEpisode = episodeOpen ? app.shows.playing : null;
  const sheetOpen =
    (inMovies && app.movies.sheetId !== null) || (inShows && app.shows.sheetId !== null);

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

  // The two the band can tell of, and how it lays them out.
  const playingItem: BandItem | null = movieOpen
    ? { kind: "movie", key: `movie:${openMovieId ?? "?"}` }
    : episodeOpen
      ? { kind: "episode", key: `show:${playingEpisode?.series.seriesId ?? "?"}` }
      : live && nowChannel
        ? { kind: "channel", key: `channel:${nowChannel.streamId}` }
        : null;
  const selectedItem: BandItem | null = selectedMovie
    ? { kind: "movie", key: `movie:${selectedMovie.movieId}` }
    : selectedShow
      ? { kind: "show", key: `show:${selectedShow.seriesId}` }
      : null;
  const layout = bandLayout({
    playing: playingItem,
    selected: selectedItem,
    sheetOpen,
    lead: app.bandLead,
  });
  const dual = layout.side === "playing" || layout.side === "selection";

  // The side column when it is the other of the two; left to the main
  // account (its context) otherwise.
  const playingSide = () =>
    movieOpen ? (
      <MovieNowSide movie={app.movies.playing} />
    ) : episodeOpen ? (
      <EpisodeNowSide playing={playingEpisode} />
    ) : nowChannel ? (
      <ChannelNowSide channel={nowChannel} onNow={onNow} now={now} />
    ) : null;
  const selectedSide = () =>
    selectedMovie ? (
      <MovieSelectedSide movie={selectedMovie} />
    ) : selectedShow ? (
      <ShowSelectedSide show={selectedShow} />
    ) : null;
  const side =
    layout.side === "playing" ? playingSide() : layout.side === "selection" ? selectedSide() : undefined;

  const selectionBand = () =>
    selectedMovie ? (
      <MovieBand key={selectedMovie.movieId} movie={selectedMovie} playing={false} dual={dual} side={side} extra={removeEntry} />
    ) : selectedShow ? (
      <ShowBand key={selectedShow.seriesId} show={selectedShow} dual={dual} side={side} extra={removeEntry} />
    ) : null;

  const playingBand = () => {
    if (movieOpen) {
      const movie =
        app.movies.playing ?? (selectedMovie?.movieId === openMovieId ? selectedMovie : null);
      return movie ? (
        <MovieBand key={movie.movieId} movie={movie} playing dual={dual} side={side} />
      ) : (
        // Open, but the title could not be looked up: still stoppable.
        <Unnamed what="Playing a movie" side={side} />
      );
    }
    if (episodeOpen) {
      return playingEpisode ? (
        <EpisodeBand key={playingEpisode.series.seriesId} playing={playingEpisode} dual={dual} side={side} />
      ) : (
        // Open, but not looked up yet: still stoppable.
        <Unnamed what="Playing an episode" side={side} />
      );
    }
    if (!nowChannel) return null;
    return (
      <ChannelBand
        mode="playing"
        channel={nowChannel}
        onNow={onNow}
        next={next}
        later={later}
        now={now}
        dual={dual}
        side={side}
        onFullScreen={toWatch}
        onStop={() => {
          void run(dispatch, "stop", backend.stop());
        }}
      />
    );
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
        ) : layout.main === null ? (
          ready && nowChannel ? (
            <div className="band one">
              <ChannelBand
                mode="ready"
                channel={nowChannel}
                onNow={onNow}
                next={next}
                later={later}
                now={now}
                onPlay={resume}
              />
            </div>
          ) : (
            <Idle picture={picture} movies={inMovies} shows={inShows} />
          )
        ) : (
          <div className={`band ${dual ? "two" : "one"}`}>
            {layout.main === "selection" ? selectionBand() : playingBand()}
          </div>
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

/** The name of the category a channel is in, once the categories are here. */
function useCategoryName(channel: ChannelDto): string | undefined {
  const { categories } = useApp();
  return channel.categoryId !== null && categories.status === "ready"
    ? categoryNames(categories.data).get(channel.categoryId)
    : undefined;
}

/** The programme first, as the mockup has it: its title, the channel with
 *  the programme's times, how far along it is, and what it is about. The
 *  category and what follows are the side column's while it shows them
 *  ("Later on"); they stay here (`chan-extra`) where it does not: beside a
 *  selection, or in a band too narrow for it. Without guide data the
 *  channel's name takes the title's place. Shared by the playing and the
 *  restored channel. */
function ChannelProgramme({
  channel,
  onNow,
  next,
  now,
  badge,
  notice,
  lead,
}: {
  channel: ChannelDto;
  onNow: ProgrammeDto | null;
  next: ProgrammeDto | null;
  now: number;
  badge?: string;
  /** Said above the title, beside the logo ("Where you left off"). */
  lead?: ReactNode;
  /** Said straight under the channel's line, before the programme's
   *  details: what must be in view without scrolling the column. */
  notice?: ReactNode;
}) {
  const { qualityNews } = useApp();
  // What playing it has since added to its record, over what the channel
  // was fetched with: found off air a moment ago, say.
  const quality = channel.streamId in qualityNews ? qualityNews[channel.streamId] : channel.quality;
  const off = offAirLabel(quality, now);
  const category = useCategoryName(channel);
  // The logo is square, so it sits beside the head only (title, channel line,
  // how far along) and what follows runs the band's full width under both.
  return (
    <>
      <div className="band-head">
        <BandLogo url={channel.logoUrl} name={channel.cleanName} />
        <div className="band-head-text">
          {lead}
          <h2 className="watch-title-h" data-tip-overflow={onNow ? onNow.title : channel.name}>
            {onNow ? onNow.title : channel.cleanName}
          </h2>
          {category ? (
            <p className="watch-cat chan-extra" data-tip-overflow="">
              {category}
            </p>
          ) : null}
          <ChannelLine channel={channel} badge={badge} onNow={onNow} off={off} />
          {onNow ? (
            <ProgressLine
              fraction={programmeProgress(onNow.start, onNow.stop, now)}
              label={`${duration(Math.max(0, onNow.stop - now))} left`}
            />
          ) : null}
        </div>
      </div>
      {notice}
      {onNow ? (
        onNow.description ? (
          <p className="watch-desc band-plot" data-tip-overflow="">
            {onNow.description}
          </p>
        ) : null
      ) : (
        // 15% EPG coverage: this is the common case, and it is not an error.
        <p className="watch-none">no guide data for this channel</p>
      )}
      {next ? (
        <p className="watch-next chan-extra" data-tip-overflow="">
          Next {hhmm(next.start)} {next.title}
        </p>
      ) : null}
    </>
  );
}

/** The channel's name and number, its badges, and the programme's times. */
function ChannelLine({
  channel,
  badge,
  onNow,
  off,
}: {
  channel: ChannelDto;
  badge?: string;
  onNow: ProgrammeDto | null;
  off: string | null;
}) {
  return (
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

/** The channel in the band's main column: playing, or restored from last
 *  time and not started (`ready`), whose one action is Resume.
 *
 *  It says "Resume" rather than starting by itself on purpose. Opening a
 *  stream may cost the only connection the provider allows, and an app that
 *  starts playing the moment it is launched is an app that makes noise when
 *  you opened it to look something up. */
function ChannelBand(
  props: {
    channel: ChannelDto;
    onNow: ProgrammeDto | null;
    next: ProgrammeDto | null;
    later: readonly ProgrammeDto[];
    now: number;
    dual?: boolean;
    /** The side column, when WatchBand fills it; else what is on later. */
    side?: ReactNode;
  } & (
    | { mode: "ready"; onPlay: () => void }
    | { mode: "playing"; onFullScreen: () => void; onStop: () => void }
  ),
) {
  const { channel, onNow, next, later, now, dual = false, side } = props;
  return (
    <>
      {props.mode === "ready" ? (
        <BandMain
          className="channel-band"
          actions={
            <>
              <button className="ctl primary" onClick={props.onPlay}>
                Resume
              </button>
              <ChannelLinks channel={channel} />
            </>
          }
        >
          <ChannelProgramme
            channel={channel}
            onNow={onNow}
            next={next}
            now={now}
            lead={<p className="watch-state">Where you left off</p>}
          />
        </BandMain>
      ) : (
        <Playing
          channel={channel}
          onNow={onNow}
          next={next}
          now={now}
          dual={dual}
          onFullScreen={props.onFullScreen}
          onStop={props.onStop}
        />
      )}
      {side !== undefined ? side : <LaterOn channel={channel} later={later} />}
    </>
  );
}

/** With the channel alone in the band: what is on after this, and the
 *  category it is in. */
function LaterOn({ channel, later }: { channel: ChannelDto; later: readonly ProgrammeDto[] }) {
  const category = useCategoryName(channel);
  if (later.length === 0 && !category) return null;
  return (
    <BandSide kind="context" label={`Later on ${channel.cleanName}`}>
      <p className="band-kicker" data-tip-overflow="">
        Later on {channel.cleanName}
      </p>
      {later.length > 0 ? (
        <div className="band-sched">
          {later.map((p) => (
            <Fragment key={`${p.start}:${p.title}`}>
              <span className="band-sched-time">{hhmm(p.start)}</span>
              <span className="band-sched-name" data-tip-overflow="">
                {p.title}
              </span>
            </Fragment>
          ))}
        </div>
      ) : (
        <p className="watch-none">no guide data for later</p>
      )}
      <span className="band-grow" />
      {category ? (
        <p className="band-side-foot" data-tip-overflow="">
          {category}
        </p>
      ) : null}
    </BandSide>
  );
}

/** The channel playing, beside a selection: its name and number, the
 *  programme and its times, how far along; ↩ Show, Full screen and Stop. */
function ChannelNowSide({ channel, onNow, now }: { channel: ChannelDto; onNow: ProgrammeDto | null; now: number }) {
  return (
    <BandSide
      kind="now"
      label="Now playing"
      swap
      actions={
        <>
          <SwapButton label="Show what is playing" />
          <PlayingEnd />
        </>
      }
    >
      <Kicker what="live" />
      <p className="band-side-t" data-tip-overflow={channel.name}>
        {channel.cleanName}
        {channel.num != null ? <span className="watch-num"> #{channel.numLabel ?? channel.num}</span> : null}
      </p>
      <p className="band-side-s" data-tip-overflow="">
        {onNow ? `${onNow.title} · ${timeRange(onNow.start, onNow.stop)}` : "no guide data"}
      </p>
      {onNow ? (
        <ProgressLine
          fraction={programmeProgress(onNow.start, onNow.stop, now)}
          label={`${duration(Math.max(0, onNow.stop - now))} left`}
        />
      ) : null}
    </BandSide>
  );
}

/** Something is open in the player that could not be looked up: its kind,
 *  and still a way to stop it. */
function Unnamed({ what, side }: { what: string; side?: ReactNode }) {
  return (
    <>
      <BandMain actions={<PlayingEnd />}>
        <h2 className="watch-title-h">{what}</h2>
      </BandMain>
      {side}
    </>
  );
}

function Playing({
  channel,
  onNow,
  next,
  now,
  dual,
  onFullScreen,
  onStop,
}: {
  channel: ChannelDto;
  onNow: ProgrammeDto | null;
  next: ProgrammeDto | null;
  now: number;
  dual: boolean;
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
    <BandMain
      className="channel-band"
      kicker={dual ? <Kicker what="live" /> : null}
      actions={
        <>
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
        </>
      }
    >
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
    </BandMain>
  );
}
