// One row of the Shows list: the poster as a thumbnail, the title with its
// badges, then the year, the seasons and the rating in their own columns, and
// at the end what has been marked. Unknown values are blank, never "0" or "?".

import { memo } from "react";
import { movieBadges } from "../logic/movies";
import { seasonsText } from "../logic/shows";
import { LIST_ROW_HEIGHT } from "../logic/virtual";
import type { SeriesDto } from "../types";
import type { VodRowProps } from "./OnDemandList";
import { Poster } from "./Poster";

export const ShowRow = memo(function ShowRow({
  item: show,
  top,
  index,
  idPrefix,
  selected,
  now,
  onSelect,
  onOpen,
}: VodRowProps<SeriesDto>) {
  const badges = movieBadges(show, now);
  return (
    // Not a button, as a movie's row is not: the list is one focus stop and
    // the arrow keys move the selection inside it.
    <div
      className={`row show-row ${selected ? "sel" : ""}`}
      id={`${idPrefix}-${show.seriesId}`}
      role="option"
      aria-selected={selected}
      style={{ top, height: LIST_ROW_HEIGHT }}
      onClick={() => onSelect(show, index)}
      onDoubleClick={() => onOpen(show, index)}
    >
      <Poster url={show.posterUrl} name={show.cleanName} className="poster row-poster" />
      <span className="row-name">
        <span className="row-name-text" data-tip-overflow={show.name}>
          {show.cleanName}
        </span>
        {badges.map((b) => (
          <span className="badge movie-badge" key={b}>
            {b}
          </span>
        ))}
      </span>
      <span className="show-row-year">{show.year ?? ""}</span>
      <span className="show-row-seasons">{seasonsText(show.seasonCount)}</span>
      <span className="show-row-rating">{show.rating !== null ? `★ ${show.rating.toFixed(1)}` : ""}</span>
      <span className="show-row-marks">
        {show.watched ? (
          <span className="show-row-mark watched" data-tip="Watched" aria-label="Watched">
            ✓
          </span>
        ) : null}
        {show.favourite ? (
          <span className="show-row-mark" data-tip="Favourite" aria-label="Favourite">
            ★
          </span>
        ) : null}
      </span>
    </div>
  );
});
