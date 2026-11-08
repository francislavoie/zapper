// One row of the Movies list: the poster as a thumbnail, the title with its
// badges, then the year, the length and the rating in their own columns, and
// at the end what has been marked. Unknown values are blank, never "0" or "?".

import { memo } from "react";
import { movieBadges, runtime } from "../logic/movies";
import { progressOf, timeLeft } from "../logic/resume";
import { LIST_ROW_HEIGHT } from "../logic/virtual";
import type { MovieDto } from "../types";
import type { VodRowProps } from "./OnDemandList";
import { Poster } from "./Poster";

export const MovieRow = memo(function MovieRow({
  item: movie,
  top,
  index,
  idPrefix,
  selected,
  now,
  onSelect,
  onOpen,
}: VodRowProps<MovieDto>) {
  const badges = movieBadges(movie, now);
  const place = {
    watched: movie.watched,
    position: movie.position ?? null,
    duration: movie.duration ?? null,
    durationSecs: movie.durationSecs,
  };
  const progress = progressOf(place);
  return (
    // Not a button: the list is one focus stop and the arrow keys move the
    // selection inside it, so a row must not take focus away when it is
    // clicked (and a focused row that scrolls out of the window would drop it).
    <div
      className={`row movie-row ${selected ? "sel" : ""}`}
      id={`${idPrefix}-${movie.movieId}`}
      role="option"
      aria-selected={selected}
      style={{ top, height: LIST_ROW_HEIGHT }}
      onClick={() => onSelect(movie, index)}
      onDoubleClick={() => onOpen(movie, index)}
    >
      <Poster url={movie.posterUrl} name={movie.cleanName} className="poster row-poster" />
      <span className="row-name">
        <span className="row-name-text" data-tip-overflow={movie.name}>
          {movie.cleanName}
        </span>
        {badges.map((b) => (
          <span className="badge movie-badge" key={b}>
            {b}
          </span>
        ))}
      </span>
      <span className="movie-year">{movie.year ?? ""}</span>
      <span className="movie-length">{runtime(movie.durationSecs)}</span>
      <span className="movie-rating">{movie.rating !== null ? `★ ${movie.rating.toFixed(1)}` : ""}</span>
      <span className="movie-progress">
        {progress !== null ? (
          <>
            <span className="movie-progress-bar" aria-hidden>
              <span style={{ width: `${Math.round(progress * 100)}%` }} />
            </span>
            <span className="movie-progress-left">{timeLeft(place)}</span>
          </>
        ) : null}
      </span>
      <span className="movie-marks">
        {movie.watched ? (
          <span className="movie-mark watched" data-tip="Watched" aria-label="Watched">
            ✓
          </span>
        ) : null}
        {movie.favourite ? (
          <span className="movie-mark" data-tip="Favourite" aria-label="Favourite">
            ★
          </span>
        ) : null}
      </span>
    </div>
  );
});
