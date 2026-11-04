// The watch band's account of a movie: the one selected in the Movies tab, or
// the one the player has open. Its facts, plot and people come from the
// details the provider gives per title, fetched once the selection has been
// still for a moment (useMovieDetails). Nothing here plays anything by being
// shown: only the Play button does.

import { playMovie, setMovieWatched, toggleMovieFavourite } from "../actions";
import { backend } from "../backend";
import { useMovieDetails } from "../hooks/useMovieDetails";
import type { Async } from "../logic/async";
import {
  DETAILS_DELAY_MS,
  languagesLine,
  movieBadges,
  movieFacts,
  peopleLine,
} from "../logic/movies";
import { run, useDispatch } from "../store";
import type { MovieDetailsDto, MovieDto } from "../types";
import { Fullscreen, Play, Stop } from "./icons";

/** The most genre tags one title shows; a provider's list can run long. */
const MAX_TAGS = 4;

export interface MovieBandProps {
  movie: MovieDto;
  /** The player has this title open. */
  playing: boolean;
}

export function MovieBand({ movie, playing }: MovieBandProps) {
  const dispatch = useDispatch();
  const details = useMovieDetails(movie, playing ? 0 : DETAILS_DELAY_MS);
  return (
    <>
      <div className="watch-body movie-band">
        <h2 className="watch-title-h movie-band-title" data-tip-overflow={movie.name}>
          {movie.cleanName}
        </h2>
        <MovieFacts movie={movie} details={details} className="movie-band-facts" />
        <MovieText details={details} />
      </div>
      <div className="watch-actions">
        <MovieActions
          movie={movie}
          playing={playing}
          onDetails={() => dispatch({ type: "movies/sheet", id: movie.movieId })}
        />
      </div>
    </>
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

/** The languages, the plot and the people: what the details add. */
function MovieText({ details }: { details: Async<MovieDetailsDto> }) {
  if (details.status === "error") return <p className="watch-none">Details unavailable</p>;
  if (details.status === "loading") return <p className="watch-none">Loading details…</p>;
  if (details.status !== "ready") return null;
  const d = details.data;
  const languages = languagesLine(d);
  const people = peopleLine(d);
  return (
    <>
      {languages ? <p className="watch-cat">{languages}</p> : null}
      {d.plot ? (
        <p className="watch-desc movie-band-plot" data-tip-overflow="">
          {d.plot}
        </p>
      ) : null}
      {people ? (
        <p className="watch-next movie-band-people" data-tip-overflow="">
          {people}
        </p>
      ) : null}
    </>
  );
}

/** Play (or, for the title that is playing, Full screen and Stop), the two
 *  marks and, in the band, Details. Play opens the full player. */
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
      {playing ? null : (
        <button className="ctl movie-play" onClick={() => void playMovie(dispatch, movie)}>
          <Play className="i" /> Play
        </button>
      )}
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
      {playing ? (
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
      ) : null}
    </>
  );
}
