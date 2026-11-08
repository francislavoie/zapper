// A movie's Details: a sheet over the rail and the list (never the band, so
// the picture and what is playing stay in view). The backdrop, the poster,
// everything the band says and in full, the same actions, and a shelf of
// "More like this". Opening it plays nothing; closing it (‹ Back, ✕, or
// Escape and the mouse's back button, which useGlobalKeys answers) returns
// focus to the list it came from.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { backend } from "../backend";
import { useMovieDetails } from "../hooks/useMovieDetails";
import { playingKind, languagesLine, peopleLine } from "../logic/movies";
import { useApp, useDispatch } from "../store";
import type { MovieDto } from "../types";
import { MovieActions, MovieFacts } from "./MovieBand";
import { Poster } from "./Poster";
import { SheetControls } from "./SheetControls";

/** How many titles the shelf offers. */
const SIMILAR = 12;

export function MovieSheet({ id }: { id: number }) {
  const { movies, player } = useApp();
  const dispatch = useDispatch();
  const root = useRef<HTMLElement | null>(null);
  // Opening a title selects it first, so this is the title whose marks the
  // store keeps current. A sheet for any other has nothing to show yet.
  const movie = movies.selected?.movieId === id ? movies.selected : null;
  const details = useMovieDetails(movie, 0);

  // Focus goes in on opening and, when the sheet goes, back to the list the
  // title was chosen in (its one focus stop: the rows are options inside it)
  // — unless the user has already put it somewhere else.
  useEffect(() => {
    root.current?.focus();
    return () => {
      const here = document.activeElement;
      if (here && here !== document.body) return;
      document.querySelector<HTMLElement>(".browse .movies-viewport")?.focus();
    };
  }, []);

  const [brokenBackdrop, setBrokenBackdrop] = useState<string | null>(null);
  const [similar, setSimilar] = useState<{ id: number; items: MovieDto[] } | null>(null);
  useEffect(() => {
    let cancelled = false;
    backend
      .similarMovies(id, SIMILAR + 1)
      .then((items) => {
        if (!cancelled) setSimilar({ id, items: items.filter((m) => m.movieId !== id).slice(0, SIMILAR) });
      })
      .catch(() => {
        if (!cancelled) setSimilar({ id, items: [] });
      });
    return () => {
      cancelled = true;
    };
  }, [id]);
  const shelf = similar?.id === id ? similar.items : [];

  // Every title's sheet starts at its top, the poster and the facts in
  // sight: on opening, and on going to another title from its shelf. Before
  // the frame is drawn, so the old title's scroll never shows under the new
  // one; and again once the title is there, as the sheet draws nothing
  // until it is.
  const shown = movie !== null;
  useLayoutEffect(() => {
    if (root.current) root.current.scrollTop = 0;
  }, [id, shown]);

  if (!movie) return null;
  const d = details.status === "ready" ? details.data : null;
  const playing = playingKind(player) === "movie" && player.onDemand?.id === id;
  const first = d?.backdropUrls[0] ?? null;
  const backdrop = first !== null && first !== brokenBackdrop ? first : null;
  const languages = d ? languagesLine(d) : "";
  const people = d ? peopleLine(d) : "";

  return (
    <section
      className="movie-sheet"
      role="dialog"
      aria-label={movie.cleanName}
      tabIndex={-1}
      ref={root}
    >
      <div className="movie-sheet-backdrop" aria-hidden>
        {backdrop ? (
          <img
            src={backdrop}
            alt=""
            decoding="async"
            onError={() => setBrokenBackdrop(backdrop)}
          />
        ) : (
          <Poster url={movie.posterUrl} name={movie.cleanName} className="movie-sheet-blur" />
        )}
      </div>
      <SheetControls onClose={() => dispatch({ type: "movies/sheet", id: null })} />
      <div className="sheet-top">
        <Poster url={movie.posterUrl} name={movie.cleanName} className="poster sheet-poster" />
        <div className="sheet-text">
          <h2 className="sheet-title" data-tip-overflow={movie.name}>
            {movie.cleanName}
          </h2>
          <MovieFacts movie={movie} details={details} className="sheet-facts" />
          {details.status === "loading" ? <p className="watch-none">Loading details…</p> : null}
          {details.status === "error" ? <p className="watch-none">Details unavailable</p> : null}
          {languages ? <p className="sheet-people">{languages}</p> : null}
          {d?.plot ? <p className="sheet-plot">{d.plot}</p> : null}
          {people ? <p className="sheet-people">{people}</p> : null}
          <div className="sheet-actions">
            <MovieActions movie={movie} playing={playing} />
          </div>
        </div>
      </div>
      {shelf.length > 0 ? (
        <div className="sheet-shelf">
          <h3 className="sheet-shelf-head">More like this</h3>
          <div className="sheet-shelf-row">
            {shelf.map((m) => (
              <button
                key={m.movieId}
                className="sheet-thumb"
                onClick={() => {
                  dispatch({ type: "movies/selectMovie", movie: m });
                  dispatch({ type: "movies/sheet", id: m.movieId });
                }}
              >
                <Poster url={m.posterUrl} name={m.cleanName} className="poster sheet-thumb-art" />
                <span className="sheet-thumb-name" data-tip-overflow={m.name}>
                  {m.cleanName}
                </span>
              </button>
            ))}
          </div>
        </div>
      ) : null}
    </section>
  );
}
