// A show's sheet: over the rail and the list (never the band, so the picture
// and what is playing stay in view), as a movie's Details are. It is about
// the show: the backdrop, the poster, everything the band says and in full,
// a line of its seasons ("2 seasons · 14 episodes · 2 watched") and the same
// actions. The episodes are in the pane beside the list (ShowPane), not
// here. Opening it plays nothing; Play does, and closes it for the full
// player. Closing it (Escape, which useGlobalKeys answers, the mouse's back
// button, ‹ Back or ✕) returns focus to where it was opened from: the list or
// the pane.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { playingKind, peopleLine } from "../logic/movies";
import { seasonsSummary } from "../logic/shows";
import { useApp, useDispatch } from "../store";
import { Poster } from "./Poster";
import { SheetControls } from "./SheetControls";
import { useShowDetailsOf } from "../hooks/useShowDetails";
import { ShowActions, ShowFacts } from "./ShowBand";

export function ShowSheet({ id }: { id: number }) {
  const { shows, player } = useApp();
  const dispatch = useDispatch();
  const root = useRef<HTMLElement | null>(null);
  // Opening a show selects it first, so this is the show whose marks the
  // store keeps current. A sheet for any other has nothing to show yet.
  const show = shows.selected?.seriesId === id ? shows.selected : null;
  const details = useShowDetailsOf(show, 0);

  // Focus goes in on opening and, when the sheet goes, back to where it was
  // (the list, or the pane's Details ›) — unless the user has already put it
  // somewhere else. Where that has gone, the list. Where it was is taken
  // while the sheet is first drawn, before focus has moved into it.
  const [from] = useState(() =>
    document.activeElement instanceof HTMLElement ? document.activeElement : null,
  );
  useEffect(() => {
    root.current?.focus();
    return () => {
      const here = document.activeElement;
      if (here && here !== document.body) return;
      if (from && from.isConnected && from !== document.body) from.focus();
      else document.querySelector<HTMLElement>(".browse .shows-viewport")?.focus();
    };
  }, [from]);

  const [brokenBackdrop, setBrokenBackdrop] = useState<string | null>(null);
  const d = details.status === "ready" ? details.data : null;

  // Every show's sheet starts at its top: on opening (the sheet is keyed by
  // the show, so another show is a new sheet), before the frame is drawn,
  // and again once the show is there, as the sheet draws nothing until it is.
  const shown = show !== null;
  useLayoutEffect(() => {
    if (root.current) root.current.scrollTop = 0;
  }, [id, shown]);

  if (!show) return null;
  const playing = playingKind(player) === "episode" && shows.playing?.series.seriesId === id;
  const first = d?.backdropUrls[0] ?? null;
  const backdrop = first !== null && first !== brokenBackdrop ? first : null;
  const people = d ? peopleLine(d) : "";
  const seasons = d ? seasonsSummary(d) : "";

  return (
    <section
      className="show-sheet"
      role="dialog"
      aria-label={show.cleanName}
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
          <Poster url={show.posterUrl} name={show.cleanName} className="movie-sheet-blur" />
        )}
      </div>
      <SheetControls onClose={() => dispatch({ type: "shows/sheet", id: null })} />
      <div className="sheet-top">
        <Poster url={show.posterUrl} name={show.cleanName} className="poster sheet-poster" />
        <div className="sheet-text">
          <h2 className="sheet-title" data-tip-overflow={show.name}>
            {show.cleanName}
          </h2>
          <ShowFacts show={show} details={details} className="sheet-facts" />
          {details.status === "loading" ? <p className="watch-none">Loading details…</p> : null}
          {details.status === "error" ? <p className="watch-none">Details unavailable</p> : null}
          {d?.plot ? <p className="sheet-plot">{d.plot}</p> : null}
          {people ? <p className="sheet-people">{people}</p> : null}
          {d ? <p className="sheet-seasons">{seasons || "No episodes yet"}</p> : null}
          <div className="sheet-actions">
            <ShowActions show={show} details={details} playing={playing} />
          </div>
        </div>
      </div>
    </section>
  );
}
