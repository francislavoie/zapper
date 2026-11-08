// A show's sheet: over the rail and the list (never the band, so the picture
// and what is playing stay in view), as a movie's Details are. The backdrop,
// the poster, everything the band says and in full, the same actions; then
// the seasons, a row of tabs with the chosen one's counts at its end, and
// that season's episodes, the next-up one highlighted. It opens on the
// season of what is next. Opening it, or choosing a season, plays nothing;
// an episode plays on Enter, a double-click or its ▶, which also closes the
// sheet for the full player. Closing it (Escape, which useGlobalKeys
// answers, or the ✕) returns focus to the Shows list.

import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { playEpisode, setEpisodeWatched } from "../actions";
import { useShowDetails } from "../hooks/useShowDetails";
import { playingKind, peopleLine } from "../logic/movies";
import { firstSeason, seasonLabel, seasonMeta } from "../logic/shows";
import { useApp, useDispatch } from "../store";
import type { EpisodeDto } from "../types";
import { EpisodeList } from "./EpisodeList";
import { Poster } from "./Poster";
import { ShowActions, ShowFacts } from "./ShowBand";

export function ShowSheet({ id }: { id: number }) {
  const { shows, player } = useApp();
  const dispatch = useDispatch();
  const root = useRef<HTMLElement | null>(null);
  // Opening a show selects it first, so this is the show whose marks the
  // store keeps current. A sheet for any other has nothing to show yet.
  const show = shows.selected?.seriesId === id ? shows.selected : null;
  const details = useShowDetails(show, 0, shows.detailsRevision[id] ?? 0);

  // Focus goes in on opening and, when the sheet goes, back to the list the
  // show was chosen in (its one focus stop: the rows are options inside it)
  // — unless the user has already put it somewhere else.
  useEffect(() => {
    root.current?.focus();
    return () => {
      const here = document.activeElement;
      if (here && here !== document.body) return;
      document.querySelector<HTMLElement>(".browse .shows-viewport")?.focus();
    };
  }, []);

  const [brokenBackdrop, setBrokenBackdrop] = useState<string | null>(null);
  const d = details.status === "ready" ? details.data : null;

  // Once the episodes are here, focus moves on to them, the cursor on what
  // is next: Enter plays it, ↑/↓ choose another, without first tabbing
  // through the actions and the seasons. Only while focus is still where
  // opening put it (on the sheet itself).
  const listed = d !== null;
  useEffect(() => {
    if (!listed || document.activeElement !== root.current) return;
    root.current?.querySelector<HTMLElement>(".ep-list")?.focus({ preventScroll: true });
  }, [listed]);

  // The season chosen, while the show still has it; else the one it opens on.
  const season =
    shows.sheetSeason !== null && d?.seasons.some((s) => s.number === shows.sheetSeason)
      ? shows.sheetSeason
      : d
        ? firstSeason(d)
        : null;
  const episodes = useMemo(
    () => (d && season !== null ? d.episodes.filter((e) => e.season === season) : []),
    [d, season],
  );

  const tabs = useRef<HTMLDivElement | null>(null);
  // The chosen tab in sight in its row, which scrolls sideways when a show
  // has more seasons than fit.
  useEffect(() => {
    tabs.current
      ?.querySelector<HTMLElement>('[aria-selected="true"]')
      ?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [season]);

  if (!show) return null;
  const playing =
    playingKind(player) === "episode" && shows.playing?.series.seriesId === id;
  const first = d?.backdropUrls[0] ?? null;
  const backdrop = first !== null && first !== brokenBackdrop ? first : null;
  const people = d ? peopleLine(d) : "";
  const seasons = d?.seasons ?? [];
  const chosen = seasons.find((s) => s.number === season) ?? null;

  const choose = (n: number) => dispatch({ type: "shows/season", number: n });
  // ← → between the seasons, Home and End to either end: focus follows the
  // choice, as a tab row's does.
  const onTabKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const at = seasons.findIndex((s) => s.number === season);
    let to: number;
    switch (e.key) {
      case "ArrowLeft":
        to = at - 1;
        break;
      case "ArrowRight":
        to = at + 1;
        break;
      case "Home":
        to = 0;
        break;
      case "End":
        to = seasons.length - 1;
        break;
      default:
        return;
    }
    // Not the player's seek as well.
    e.preventDefault();
    e.stopPropagation();
    const next = seasons[Math.max(0, Math.min(seasons.length - 1, to))];
    if (!next) return;
    choose(next.number);
    requestAnimationFrame(() =>
      tabs.current?.querySelector<HTMLElement>(`[data-season="${next.number}"]`)?.focus(),
    );
  };

  const play = (episode: EpisodeDto) => void playEpisode(dispatch, { series: show, episode });
  const mark = (episode: EpisodeDto, watched: boolean) =>
    void setEpisodeWatched(dispatch, episode, watched);

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
      <button
        className="ctl icon movie-sheet-close"
        aria-label="Close"
        data-tip="Close (Esc)"
        onClick={() => dispatch({ type: "shows/sheet", id: null })}
      >
        ✕
      </button>
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
          <div className="sheet-actions">
            <ShowActions show={show} details={details} playing={playing} />
          </div>
        </div>
      </div>
      {seasons.length > 0 ? (
        <div className="seasons">
          <div
            className="seasons-tabs"
            role="tablist"
            aria-label="Seasons"
            ref={tabs}
            onKeyDown={onTabKey}
          >
            {seasons.map((s) => (
              <button
                key={s.number}
                className={`seasons-tab ${s.number === season ? "on" : ""}`}
                role="tab"
                aria-selected={s.number === season}
                tabIndex={s.number === season ? 0 : -1}
                data-season={s.number}
                data-tip-overflow={s.name}
                onClick={() => choose(s.number)}
              >
                {s.name || seasonLabel(s.number)}
              </button>
            ))}
          </div>
          {chosen ? <span className="season-meta">{seasonMeta(chosen)}</span> : null}
        </div>
      ) : null}
      {d && seasons.length === 0 ? <p className="show-sheet-none">No episodes yet</p> : null}
      {d && season !== null ? (
        <div className="show-sheet-eps" role="tabpanel" aria-label={chosen?.name ?? seasonLabel(season)}>
          <EpisodeList
            key={`${id}:${season}`}
            episodes={episodes}
            nextUp={d.nextUp}
            onPlay={play}
            onWatched={mark}
          />
        </div>
      ) : null}
    </section>
  );
}
