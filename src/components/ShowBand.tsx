// The watch band's account of a show: the one selected in the Shows tab, or
// the one whose episode the player has open. Its facts, plot and people come
// from the details the provider gives per show, fetched once the selection
// has been still for a moment (useShowDetails); so does the main action,
// which plays what is next. Nothing here plays anything by being shown: only
// the Play button does.

import { playEpisode, setEpisodeWatched, setShowWatched, toggleShowFavourite } from "../actions";
import { useShowDetails } from "../hooks/useShowDetails";
import type { Async } from "../logic/async";
import { DETAILS_DELAY_MS, movieBadges, peopleLine } from "../logic/movies";
import { episodeLine, showAction, showFacts } from "../logic/shows";
import { useApp, useDispatch } from "../store";
import type { PlayingEpisodeDto, SeriesDetailsDto, SeriesDto } from "../types";
import { PlayingEnd } from "./MovieBand";
import { Play } from "./icons";

/** The most genre tags one show shows, as a movie's. */
const MAX_TAGS = 4;

export interface ShowBandProps {
  show: SeriesDto;
  /** The player has an episode of this show open. */
  playing: PlayingEpisodeDto | null;
}

export function ShowBand({ show, playing }: ShowBandProps) {
  const { shows } = useApp();
  // The episode playing says what the band needs; the show's details are
  // not asked for while it does.
  const details = useShowDetails(
    playing ? null : show,
    DETAILS_DELAY_MS,
    shows.detailsRevision[show.seriesId] ?? 0,
  );
  const openSheet = useOpenSheet(show);
  if (playing) return <PlayingEpisode show={show} playing={playing} onDetails={openSheet} />;
  const d = details.status === "ready" ? details.data : null;
  const people = d ? peopleLine(d) : "";
  return (
    <>
      <div className="watch-body show-band">
        <h2 className="watch-title-h show-band-title" data-tip-overflow={show.name}>
          {show.cleanName}
        </h2>
        <ShowFacts show={show} details={details} className="show-band-facts" />
        {details.status === "error" ? <p className="watch-none">Details unavailable</p> : null}
        {details.status === "loading" ? <p className="watch-none">Loading details…</p> : null}
        {d?.plot ? (
          <p className="watch-desc show-band-plot" data-tip-overflow="">
            {d.plot}
          </p>
        ) : null}
        {people ? (
          <p className="watch-next show-band-people" data-tip-overflow="">
            {people}
          </p>
        ) : null}
      </div>
      <div className="watch-actions">
        <ShowActions show={show} details={details} playing={false} onDetails={openSheet} />
      </div>
    </>
  );
}

/** Details: the show's sheet, in the Shows tab (from Live TV, where the
 *  episode playing is shown, it goes there). */
function useOpenSheet(show: SeriesDto): () => void {
  const { section, shows } = useApp();
  const dispatch = useDispatch();
  return () => {
    if (section !== "shows") dispatch({ type: "section", section: "shows" });
    if (shows.selected?.seriesId !== show.seriesId) dispatch({ type: "shows/selectShow", show });
    dispatch({ type: "shows/sheet", id: show.seriesId });
  };
}

/** The episode the player has open: its show, "S2 E4 · Night Shift · 52m",
 *  its plot, and the actions of what is playing. */
function PlayingEpisode({
  show,
  playing,
  onDetails,
}: {
  show: SeriesDto;
  playing: PlayingEpisodeDto;
  onDetails: () => void;
}) {
  const dispatch = useDispatch();
  const { episode } = playing;
  return (
    <>
      <div className="watch-body show-band">
        <h2 className="watch-title-h show-band-title" data-tip-overflow={show.name}>
          {show.cleanName}
        </h2>
        <p className="show-band-facts">{episodeLine(episode)}</p>
        {episode.plot ? (
          <p className="watch-desc show-band-plot" data-tip-overflow="">
            {episode.plot}
          </p>
        ) : null}
      </div>
      <div className="watch-actions">
        <button
          className="ctl"
          onClick={() => void setEpisodeWatched(dispatch, episode, !episode.watched)}
        >
          {episode.watched ? "Mark unwatched" : "Mark watched"}
        </button>
        <FavouriteButton show={show} />
        <button className="ctl" onClick={onDetails}>
          Details
        </button>
        <PlayingEnd />
      </div>
    </>
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

/** ▶ Play S2 E4 (what is next; disabled, saying so, for a show without
 *  episodes; or, while an episode of it plays, Full screen and Stop), ★,
 *  Mark watched and, in the band, Details. Play opens the full player and
 *  closes the sheet. */
export function ShowActions({
  show,
  details,
  playing,
  onDetails,
}: {
  show: SeriesDto;
  details: Async<SeriesDetailsDto>;
  playing: boolean;
  onDetails?: () => void;
}) {
  const dispatch = useDispatch();
  const d = details.status === "ready" ? details.data : null;
  const action = d ? showAction(d) : null;
  const episode = action && d ? d.episodes.find((e) => e.episodeId === action.episodeId) : undefined;
  return (
    <>
      {playing ? null : (
        <button
          className="ctl movie-play show-play"
          disabled={!episode}
          onClick={() => {
            if (episode) void playEpisode(dispatch, { series: show, episode });
          }}
        >
          <Play className="i" /> {action ? action.label : "Play"}
        </button>
      )}
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
    </>
  );
}
