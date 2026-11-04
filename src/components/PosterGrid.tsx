// The Movies list as posters: a grid of cards, windowed by rows the way the
// list is windowed by titles, so only what is on screen (and a little either
// side) exists as DOM. The scrolling element and the geometry belong to
// MoviesList, which also needs them for the keyboard and for asking the right
// pages to be fetched; this draws the visible cards where it is told.

import { memo } from "react";
import { POSTER_GAP, movieBadges, runtime } from "../logic/movies";
import type { posterGrid } from "../logic/movies";
import type { MovieDto } from "../types";
import { Poster } from "./Poster";

export type PosterGeometry = ReturnType<typeof posterGrid>;

export interface PosterGridProps {
  /** Which rows are drawn, and the column and card sizes. */
  geometry: PosterGeometry;
  /** The first and one past the last row to draw. */
  rows: { start: number; end: number };
  total: number;
  rowAt: (index: number) => MovieDto | undefined;
  selectedId: number | null;
  now: number;
  onSelect: (m: MovieDto) => void;
  onOpen: (m: MovieDto) => void;
}

export function PosterGrid({
  geometry,
  rows,
  total,
  rowAt,
  selectedId,
  now,
  onSelect,
  onOpen,
}: PosterGridProps) {
  const { columns, cardWidth, rowHeight } = geometry;
  const cards = [];
  for (let r = rows.start; r < rows.end; r++) {
    for (let c = 0; c < columns; c++) {
      const i = r * columns + c;
      if (i >= total) break;
      const m = rowAt(i);
      const left = POSTER_GAP + c * (cardWidth + POSTER_GAP);
      const top = POSTER_GAP + r * rowHeight;
      cards.push(
        m ? (
          <Card
            key={m.movieId}
            movie={m}
            left={left}
            top={top}
            width={cardWidth}
            selected={m.movieId === selectedId}
            now={now}
            onSelect={onSelect}
            onOpen={onOpen}
          />
        ) : (
          <div key={`gap-${i}`} className="poster-card gap" style={{ left, top, width: cardWidth }} />
        ),
      );
    }
  }
  return <>{cards}</>;
}

const Card = memo(function Card({
  movie,
  left,
  top,
  width,
  selected,
  now,
  onSelect,
  onOpen,
}: {
  movie: MovieDto;
  left: number;
  top: number;
  width: number;
  selected: boolean;
  now: number;
  onSelect: (m: MovieDto) => void;
  onOpen: (m: MovieDto) => void;
}) {
  const badges = movieBadges(movie, now);
  const facts = [
    movie.year ? String(movie.year) : "",
    runtime(movie.durationSecs),
    movie.rating !== null ? `★ ${movie.rating.toFixed(1)}` : "",
  ].filter((s) => s !== "");
  return (
    <div
      className={`poster-card ${selected ? "sel" : ""}`}
      id={`movie-${movie.movieId}`}
      role="option"
      aria-selected={selected}
      style={{ left, top, width }}
      onClick={() => onSelect(movie)}
      onDoubleClick={() => onOpen(movie)}
    >
      <div className="poster-art" style={{ height: Math.round(width * 1.5) }}>
        <Poster url={movie.posterUrl} name={movie.cleanName} className="poster poster-img" />
        {badges.length > 0 ? (
          <span className="poster-badges">
            {badges.map((b) => (
              <span className="badge movie-badge" key={b}>
                {b}
              </span>
            ))}
          </span>
        ) : null}
        {movie.watched ? (
          <span className="poster-watched" data-tip="Watched" aria-label="Watched">
            ✓
          </span>
        ) : null}
        <span className="poster-title" data-tip-overflow={movie.name}>
          {movie.cleanName}
        </span>
      </div>
      <span className="poster-meta">{facts.join(" · ")}</span>
    </div>
  );
});
