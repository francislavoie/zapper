// One row of the Movies list: the poster as a thumbnail, the title with its
// badges, then the year, the length and the rating in their own columns, and
// at the end what has been marked. Unknown values are blank, never "0" or "?".

import { memo } from "react";
import { movieBadges, runtime } from "../logic/movies";
import { LIST_ROW_HEIGHT } from "../logic/virtual";
import type { MovieDto } from "../types";
import { Poster } from "./Poster";

export interface MovieRowProps {
  movie: MovieDto;
  top: number;
  selected: boolean;
  /** Unix seconds, for the NEW badge; moves on slowly, so rows keep their
   *  memoised render between its ticks. */
  now: number;
  onSelect: (m: MovieDto) => void;
  onOpen: (m: MovieDto) => void;
}

export const MovieRow = memo(function MovieRow({
  movie,
  top,
  selected,
  now,
  onSelect,
  onOpen,
}: MovieRowProps) {
  const badges = movieBadges(movie, now);
  return (
    // Not a button: the list is one focus stop and the arrow keys move the
    // selection inside it, so a row must not take focus away when it is
    // clicked (and a focused row that scrolls out of the window would drop it).
    <div
      className={`row movie-row ${selected ? "sel" : ""}`}
      id={`movie-${movie.movieId}`}
      role="option"
      aria-selected={selected}
      style={{ top, height: LIST_ROW_HEIGHT }}
      onClick={() => onSelect(movie)}
      onDoubleClick={() => onOpen(movie)}
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
