// The watch band's account of a show: the one selected in the Shows tab
// (ShowBand), or the one whose episode the player has open (EpisodeBand). Its
// facts, plot and people come from the details the provider gives per show,
// fetched once the selection has been still for a moment (useShowDetails); so
// does the main action, which plays what is next. Nothing here plays anything
// by being shown: only the Play button does.
//
// The side column is what WatchBand puts there (what is playing, beside a
// selection) or, with the show alone in the band, what is next: the episode
// Play would start, or the one after the episode playing (the same
// `nextEpisode` the Up next card counts down to).

import { useMemo, type ReactNode } from "react";
import { playEpisode, setEpisodeWatched, setShowWatched, toggleShowFavourite } from "../actions";
import { useOpenShowSheet } from "../hooks/useOpenShowSheet";
import { useShowDetailsOf } from "../hooks/useShowDetails";
import type { Async } from "../logic/async";
import { clock, playedFraction, positionLabel } from "../logic/format";
import { DETAILS_DELAY_MS, movieBadges, peopleLine, runtime } from "../logic/movies";
import {
  episodeCode,
  episodeLine,
  episodeProgress,
  episodeTitle,
  seasonLabel,
  showAction,
  showFacts,
  watchedSummary,
} from "../logic/shows";
import { showPlayStart } from "../logic/resume";
import { nextEpisode } from "../logic/upNext";
import { useApp, useDispatch } from "../store";
import type { EpisodeDto, PlayingEpisodeDto, SeriesDetailsDto, SeriesDto } from "../types";
import { BandMain, BandPoster, BandSide, Credits, Kicker, ProgressLine, SwapButton } from "./BandParts";
import { PlayingEnd } from "./MovieBand";
import { Play } from "./icons";

/** The most genre tags one show shows, as a movie's. */
const MAX_TAGS = 4;

export interface ShowBandProps {
  show: SeriesDto;
  /** The band tells of two things, this one in its main column. */
  dual?: boolean;
  /** The side column, when WatchBand fills it; else what is up next. */
  side?: ReactNode;
  /** Another action at the end of the row (Continue watching's Remove). */
  extra?: ReactNode;
}

/** A show selected in the list (and not playing). */
export function ShowBand({ show, dual = false, side, extra }: ShowBandProps) {
  const details = useShowDetailsOf(show, DETAILS_DELAY_MS);
  const openSheet = useOpenShowSheet(show);
  const d = details.status === "ready" ? details.data : null;
  const people = d ? peopleLine(d) : "";
  return (
    <>
      <BandMain
        className="show-band"
        thumb={<BandPoster url={show.posterUrl} name={show.cleanName} />}
        kicker={dual ? <Kicker what="selected" /> : null}
        actions={<ShowActions show={show} details={details} playing={false} onDetails={openSheet} extra={extra} />}
        credits={
          d ? (
            <Credits
              items={[
                ["With", d.cast.slice(0, 6).join(", ")],
                ["Created by", d.director ?? ""],
                ["Progress", watchedSummary(d)],
              ]}
            />
          ) : null
        }
      >
        <h2 className="watch-title-h show-band-title" data-tip-overflow={show.name}>
          {show.cleanName}
        </h2>
        <ShowFacts show={show} details={details} className="show-band-facts" />
        {details.status === "error" ? <p className="watch-none">Details unavailable</p> : null}
        {details.status === "loading" ? <p className="watch-none">Loading details…</p> : null}
        {d?.plot ? (
          <p className={`watch-desc band-plot ${dual ? "" : "long"}`} data-tip-overflow="">
            {d.plot}
          </p>
        ) : null}
        {people && !dual ? (
          <p className="watch-next band-people" data-tip-overflow="">
            {people}
          </p>
        ) : null}
      </BandMain>
      {side !== undefined ? side : <ShowUpNext details={d} />}
    </>
  );
}

/** With the show alone in the band: the episode its Play starts, how far
 *  into it, and how much of the show is watched. */
function ShowUpNext({ details: d }: { details: SeriesDetailsDto | null }) {
  const action = d ? showAction(d) : null;
  const e = action && d ? d.episodes.find((x) => x.episodeId === action.episodeId) : undefined;
  if (!d || !e) return null;
  const progress = episodeProgress(e);
  const length = runtime(e.durationSecs ?? e.duration);
  const left = e.position !== null && progress !== null ? `${clock(e.position)} of ${clock(e.duration ?? e.durationSecs ?? 0)}` : "";
  return (
    <BandSide kind="context" label="Up next">
      <p className="band-kicker">{d.nextUp !== null ? "Up next" : "All watched"}</p>
      <p className="band-side-t" data-tip-overflow="">
        {episodeCode(e.season, e.episode, e.episodeEnd)} · {episodeTitle(e)}
      </p>
      <p className="band-side-s">{[length, left].filter(Boolean).join(" · ")}</p>
      {progress !== null ? <ProgressLine fraction={progress} /> : null}
      <span className="band-grow" />
      <p className="band-side-foot">{watchedSummary(d)}</p>
    </BandSide>
  );
}

export interface EpisodeBandProps {
  playing: PlayingEpisodeDto;
  dual?: boolean;
  /** The side column, when WatchBand fills it; else the next episode. */
  side?: ReactNode;
}

/** The episode the player has open: its show, "S2 E4 · Night Shift · 52m",
 *  how far in, its plot, and the actions of what is playing. */
export function EpisodeBand({ playing, dual = false, side }: EpisodeBandProps) {
  const dispatch = useDispatch();
  const { player } = useApp();
  const show = playing.series;
  const { episode } = playing;
  const openSheet = useOpenShowSheet(show);
  // Read for the next episode; Up next has usually asked already.
  const details = useShowDetailsOf(side !== undefined ? null : show, 0);
  const at = player.onDemand?.kind === "episode" ? player.onDemand : null;
  return (
    <>
      <BandMain
        className="show-band"
        thumb={<BandPoster url={show.posterUrl} name={show.cleanName} progress={at ? playedFraction(at) : null} />}
        kicker={dual ? <Kicker what="playing" /> : null}
        actions={
          <>
            <button
              className="ctl"
              onClick={() => void setEpisodeWatched(dispatch, episode, !episode.watched)}
            >
              {episode.watched ? "Mark unwatched" : "Mark watched"}
            </button>
            <FavouriteButton show={show} />
            <button className="ctl" onClick={openSheet}>
              Details
            </button>
            <PlayingEnd />
          </>
        }
      >
        <h2 className="watch-title-h show-band-title" data-tip-overflow={show.name}>
          {show.cleanName}
        </h2>
        <p className="band-line" data-tip-overflow="">
          {episodeLine(episode)}
        </p>
        {at ? <ProgressLine fraction={playedFraction(at)} label={positionLabel(at)} /> : null}
        {episode.plot ? (
          <p className="watch-desc band-plot" data-tip-overflow="">
            {episode.plot}
          </p>
        ) : null}
      </BandMain>
      {side !== undefined ? (
        side
      ) : (
        <NextEpisode details={details.status === "ready" ? details.data : null} current={episode} />
      )}
    </>
  );
}

/** With the episode playing alone in the band: the one after it (what Up
 *  next will offer), and how much of this season is watched. */
function NextEpisode({ details: d, current }: { details: SeriesDetailsDto | null; current: EpisodeDto }) {
  const next = useMemo(() => (d ? nextEpisode(d.episodes, current.episodeId) : null), [d, current.episodeId]);
  if (!d) return null;
  const season = d.seasons.find((s) => s.number === current.season);
  const foot = season
    ? `${season.name || seasonLabel(season.number)} · ${season.watchedCount} of ${season.episodeCount} watched`
    : "";
  return (
    <BandSide kind="context" label="Next episode">
      <p className="band-kicker">Next episode</p>
      {next ? (
        <>
          <p className="band-side-t" data-tip-overflow="">
            {episodeCode(next.season, next.episode, next.episodeEnd)} · {episodeTitle(next)}
          </p>
          <p className="band-side-s">
            {[runtime(next.durationSecs ?? next.duration), "plays after this one"].filter(Boolean).join(" · ")}
          </p>
        </>
      ) : (
        <p className="band-side-s">This is the last one.</p>
      )}
      <span className="band-grow" />
      {foot ? <p className="band-side-foot">{foot}</p> : null}
    </BandSide>
  );
}

/** The episode the player has open, beside a selection: its show, its code
 *  and title, how far in; ↩ Show, Full screen and Stop. */
export function EpisodeNowSide({ playing }: { playing: PlayingEpisodeDto | null }) {
  const { player } = useApp();
  const at = player.onDemand?.kind === "episode" ? player.onDemand : null;
  const e = playing?.episode;
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
      <Kicker what="playing" />
      <p className="band-side-t" data-tip-overflow={playing?.series.name ?? ""}>
        {playing?.series.cleanName ?? "An episode"}
      </p>
      {e ? (
        <p className="band-side-s" data-tip-overflow="">
          {episodeCode(e.season, e.episode, e.episodeEnd)} · {episodeTitle(e)}
        </p>
      ) : null}
      {at ? <ProgressLine fraction={playedFraction(at)} label={positionLabel(at)} /> : null}
    </BandSide>
  );
}

/** The selected show, beside what plays once ↩ Show has put that first:
 *  its name and facts, Play (what is next), and ↩ Show back. */
export function ShowSelectedSide({ show }: { show: SeriesDto }) {
  const details = useShowDetailsOf(show, DETAILS_DELAY_MS);
  return (
    <BandSide
      kind="now"
      label="Selected"
      swap
      actions={
        <>
          <ShowPlayButton show={show} details={details} />
          <SwapButton label="Show the selection" />
        </>
      }
    >
      <Kicker what="selected" />
      <p className="band-side-t" data-tip-overflow={show.name}>
        {show.cleanName}
      </p>
      <p className="band-side-s">{showFacts(show)}</p>
    </BandSide>
  );
}

/** The years, seasons and rating, the genres as short outlined words, and
 *  4K where the name says so. */
export function ShowFacts({
  show,
  details,
  className,
}: {
  show: SeriesDto;
  details: Async<SeriesDetailsDto>;
  className: string;
}) {
  const d = details.status === "ready" ? details.data : undefined;
  const genres = d ? d.genres.slice(0, MAX_TAGS) : [];
  const uhd = movieBadges(show, 0).includes("4K");
  return (
    <div className={className}>
      <span>{showFacts(show, d)}</span>
      {genres.map((g) => (
        <span className="movie-tag" key={g} data-tip-overflow="">
          {g}
        </span>
      ))}
      {uhd ? <span className="badge movie-badge">4K</span> : null}
    </div>
  );
}

function FavouriteButton({ show }: { show: SeriesDto }) {
  const dispatch = useDispatch();
  return (
    <button
      className={`ctl ${show.favourite ? "on" : ""}`}
      aria-pressed={show.favourite}
      onClick={() => void toggleShowFavourite(dispatch, show)}
    >
      ★ {show.favourite ? "Favourited" : "Favourite"}
    </button>
  );
}

/** ▶ Play S2 E4: what is next (disabled for a show without episodes). Opens
 *  the full player and closes the sheet. A place given to it since its
 *  details were read (on another device) is asked about, not played over. */
export function ShowPlayButton({ show, details }: { show: SeriesDto; details: Async<SeriesDetailsDto> }) {
  const dispatch = useDispatch();
  const d = details.status === "ready" ? details.data : null;
  const action = d ? showAction(d) : null;
  const episode = action && d ? d.episodes.find((e) => e.episodeId === action.episodeId) : undefined;
  return (
    <button
      className="ctl primary movie-play show-play"
      disabled={!episode}
      onClick={() => {
        if (episode) void playEpisode(dispatch, { series: show, episode }, showPlayStart(action?.resume ?? false));
      }}
    >
      <Play className="i" /> {action ? action.label : "Play"}
    </button>
  );
}

/** A show's action row, in the band and on its sheet: ▶ Play S2 E4 (or,
 *  while an episode of it plays, Full screen and Stop), ★, Mark watched and,
 *  in the band, Details. The one place it is built. */
export function ShowActions({
  show,
  details,
  playing,
  onDetails,
  main,
  extra,
}: {
  show: SeriesDto;
  details: Async<SeriesDetailsDto>;
  playing: boolean;
  onDetails?: () => void;
  /** In place of ▶ Play: the episode pane's own main action. */
  main?: ReactNode;
  /** Another action at the end of the row. */
  extra?: ReactNode;
}) {
  const dispatch = useDispatch();
  const d = details.status === "ready" ? details.data : null;
  const action = d ? showAction(d) : null;
  return (
    <>
      {main ?? (playing ? null : <ShowPlayButton show={show} details={details} />)}
      {d && !action ? <span className="show-band-none">No episodes yet</span> : null}
      <FavouriteButton show={show} />
      <button className="ctl" onClick={() => void setShowWatched(dispatch, show, !show.watched)}>
        {show.watched ? "Mark unwatched" : "Mark watched"}
      </button>
      {onDetails ? (
        <button className="ctl" onClick={onDetails}>
          Details
        </button>
      ) : null}
      {playing ? <PlayingEnd /> : null}
      {extra}
    </>
  );
}
