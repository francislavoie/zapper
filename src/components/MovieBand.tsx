// The watch band's account of a movie: the one selected in the Movies tab, or
// the one the player has open. Its facts, plot and people come from the
// details the provider gives per title, fetched once the selection has been
// still for a moment (useMovieDetails). Nothing here plays anything by being
// shown: only the Play button does.
//
// The band's main column is the movie; its side column is either what
// WatchBand puts there (what is playing, beside a selection) or, with the
// movie alone in the band, its audio and subtitles.

import type { ReactNode } from "react";
import { playMovie, setMovieWatched, toggleMovieFavourite } from "../actions";
import { backend } from "../backend";
import { useMovieDetails } from "../hooks/useMovieDetails";
import type { Async } from "../logic/async";
import { playedFraction, positionLabel } from "../logic/format";
import {
  DETAILS_DELAY_MS,
  languagesLine,
  movieBadges,
  movieFacts,
  peopleLine,
} from "../logic/movies";
import { run, useApp, useDispatch } from "../store";
import type { MovieDetailsDto, MovieDto } from "../types";
import { BandMain, BandPoster, BandSide, Credits, Kicker, ProgressLine, SwapButton } from "./BandParts";
import { Fullscreen, Play, Stop } from "./icons";

/** The most genre tags one title shows; a provider's list can run long. */
const MAX_TAGS = 4;

export interface MovieBandProps {
  movie: MovieDto;
  /** The player has this title open. */
  playing: boolean;
  /** The band tells of two things, this one in its main column. */
  dual?: boolean;
  /** The side column, when WatchBand fills it; else the movie's languages. */
  side?: ReactNode;
}

export function MovieBand({ movie, playing, dual = false, side }: MovieBandProps) {
  const dispatch = useDispatch();
  const { player } = useApp();
  const details = useMovieDetails(movie, playing ? 0 : DETAILS_DELAY_MS);
  const d = details.status === "ready" ? details.data : null;
  const at = playing && player.onDemand?.kind === "movie" ? player.onDemand : null;
  const people = d ? peopleLine(d) : "";
  return (
    <>
      <BandMain
        className="movie-band"
        thumb={<BandPoster url={movie.posterUrl} name={movie.cleanName} progress={at ? playedFraction(at) : null} />}
        kicker={dual ? <Kicker what={playing ? "playing" : "selected"} /> : null}
        actions={
          <MovieActions
            movie={movie}
            playing={playing}
            onDetails={() => dispatch({ type: "movies/sheet", id: movie.movieId })}
          />
        }
        credits={
          d ? (
            <Credits
              items={[
                ["Director", d.director ?? ""],
                ["With", d.cast.slice(0, 6).join(", ")],
                ["Audio · subtitles", languagesLine(d)],
              ]}
            />
          ) : null
        }
      >
        <h2 className="watch-title-h movie-band-title" data-tip-overflow={movie.name}>
          {movie.cleanName}
        </h2>
        <MovieFacts movie={movie} details={details} className="movie-band-facts" />
        {at ? <ProgressLine fraction={playedFraction(at)} label={positionLabel(at)} /> : null}
        <DetailsState details={details} />
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
      {side !== undefined ? side : <MovieLanguages details={details} />}
    </>
  );
}

function DetailsState({ details }: { details: Async<unknown> }) {
  if (details.status === "error") return <p className="watch-none">Details unavailable</p>;
  if (details.status === "loading") return <p className="watch-none">Loading details…</p>;
  return null;
}

/** With the movie alone in the band: what it can be heard and read in. */
function MovieLanguages({ details }: { details: Async<MovieDetailsDto> }) {
  const line = details.status === "ready" ? languagesLine(details.data) : "";
  if (!line) return null;
  return (
    <BandSide kind="context" label="Audio and subtitles">
      <p className="band-kicker">Audio &amp; subtitles</p>
      <p className="band-side-s wrap">{line}</p>
    </BandSide>
  );
}

/** The movie the player has open, beside a selection: its name and how far
 *  in, ↩ Show, Full screen and Stop. */
export function MovieNowSide({ movie }: { movie: MovieDto | null }) {
  const { player } = useApp();
  const at = player.onDemand?.kind === "movie" ? player.onDemand : null;
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
      <p className="band-side-t" data-tip-overflow={movie?.name ?? ""}>
        {movie?.cleanName ?? "A movie"}
      </p>
      {movie ? <p className="band-side-s">{movieFacts(movie)}</p> : null}
      {at ? <ProgressLine fraction={playedFraction(at)} label={positionLabel(at)} /> : null}
    </BandSide>
  );
}

/** The selected movie, beside what plays once ↩ Show has put that first:
 *  its name and facts, Play, and ↩ Show back. */
export function MovieSelectedSide({ movie }: { movie: MovieDto }) {
  return (
    <BandSide
      kind="now"
      label="Selected"
      swap
      actions={
        <>
          <MoviePlayButton movie={movie} />
          <SwapButton label="Show the selection" />
        </>
      }
    >
      <Kicker what="selected" />
      <p className="band-side-t" data-tip-overflow={movie.name}>
        {movie.cleanName}
      </p>
      <p className="band-side-s">{movieFacts(movie)}</p>
    </BandSide>
  );
}

/** The year, length and rating, the genres as short outlined words and 4K
 *  where the name says so. */
export function MovieFacts({
  movie,
  details,
  className,
}: {
  movie: MovieDto;
  details: Async<MovieDetailsDto>;
  className: string;
}) {
  const genres = details.status === "ready" ? details.data.genres.slice(0, MAX_TAGS) : [];
  const uhd = movieBadges(movie, 0).includes("4K");
  return (
    <div className={className}>
      <span>{movieFacts(movie)}</span>
      {genres.map((g) => (
        <span className="movie-tag" key={g} data-tip-overflow="">
          {g}
        </span>
      ))}
      {uhd ? <span className="badge movie-badge">4K</span> : null}
    </div>
  );
}

/** ▶ Play: opens the full player, and closes the sheet it may be on. */
export function MoviePlayButton({ movie }: { movie: MovieDto }) {
  const dispatch = useDispatch();
  return (
    <button
      className="ctl primary movie-play"
      onClick={() => {
        // Play takes the window over; the sheet would only be under it.
        dispatch({ type: "movies/sheet", id: null });
        void playMovie(dispatch, movie);
      }}
    >
      <Play className="i" /> Play
    </button>
  );
}

/** A movie's action row, in the band and on its sheet: Play (or, for the
 *  title that is playing, Full screen and Stop), the two marks and, in the
 *  band, Details. The one place it is built. */
export function MovieActions({
  movie,
  playing,
  onDetails,
}: {
  movie: MovieDto;
  playing: boolean;
  onDetails?: () => void;
}) {
  const dispatch = useDispatch();
  return (
    <>
      {playing ? null : <MoviePlayButton movie={movie} />}
      <button
        className={`ctl ${movie.favourite ? "on" : ""}`}
        aria-pressed={movie.favourite}
        onClick={() => void toggleMovieFavourite(dispatch, movie)}
      >
        ★ {movie.favourite ? "Favourited" : "Favourite"}
      </button>
      <button className="ctl" onClick={() => void setMovieWatched(dispatch, movie, !movie.watched)}>
        {movie.watched ? "Mark unwatched" : "Mark watched"}
      </button>
      {onDetails ? (
        <button className="ctl" onClick={onDetails}>
          Details
        </button>
      ) : null}
      {playing ? <PlayingEnd /> : null}
    </>
  );
}

/** Full screen and Stop, at the end of the actions of what is playing. */
export function PlayingEnd() {
  const dispatch = useDispatch();
  return (
    <span className="watch-actions-end">
      <button
        className="ctl icon"
        aria-label="Full screen"
        data-tip="Full screen (F)"
        onClick={() => dispatch({ type: "mode", mode: "watch" })}
      >
        <Fullscreen className="i" />
      </button>
      <button
        className="ctl icon"
        aria-label="Stop"
        data-tip="Stop"
        onClick={() => void run(dispatch, "stop", backend.stop())}
      >
        <Stop className="i" />
      </button>
    </span>
  );
}
