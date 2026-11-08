// One row of Continue watching: the poster (the show's, for an episode), the
// title with the episode's code, the year, the length and the rating, then
// how far along it is and when it was last watched. Unknown values are
// blank, never "0" or "?".

import { memo } from "react";
import { continueCode, continueKey, continuePlace, continueTitle } from "../logic/continueWatching";
import { runtime } from "../logic/format";
import { progressOf, timeLeft, watchedWhen } from "../logic/resume";
import { LIST_ROW_HEIGHT } from "../logic/virtual";
import type { ContinueItemDto } from "../types";
import type { VodRowProps } from "./OnDemandList";
import { Poster } from "./Poster";

export const ContinueRow = memo(function ContinueRow({
  item,
  top,
  index,
  idPrefix,
  selected,
  playing,
  now,
  onSelect,
  onOpen,
  onMenu,
}: VodRowProps<ContinueItemDto>) {
  const title = item.kind === "movie" ? item.movie : item.series;
  const place = continuePlace(item);
  const progress = progressOf(place);
  const code = continueCode(item);
  const rating = title.rating;
  return (
    // Not a button, as the other lists' rows: the list is one focus stop.
    <div
      className={`row cw-row ${selected ? "sel" : ""} ${playing ? "playing" : ""}`}
      id={`${idPrefix}-${continueKey(item)}`}
      role="option"
      aria-selected={selected}
      style={{ top, height: LIST_ROW_HEIGHT }}
      onClick={() => onSelect(item, index)}
      onDoubleClick={() => onOpen(item, index)}
      onContextMenu={
        onMenu
          ? (e) => {
              e.preventDefault();
              onMenu(item, index, { x: e.clientX, y: e.clientY });
            }
          : undefined
      }
    >
      <Poster url={title.posterUrl} name={continueTitle(item)} className="poster row-poster" />
      <span className="row-name">
        <span className="row-name-text" data-tip-overflow={title.name}>
          {continueTitle(item)}
        </span>
        {code !== "" ? <span className="badge cw-row-code">{code}</span> : null}
      </span>
      <span className="cw-row-fig">{title.year ?? ""}</span>
      <span className="cw-row-fig">{runtime(place.duration ?? place.durationSecs)}</span>
      <span className="cw-row-fig">{rating !== null ? `★ ${rating.toFixed(1)}` : ""}</span>
      {progress !== null ? (
        <span className="cw-row-progress">
          <span className="cw-row-bar" aria-hidden>
            <span style={{ width: `${Math.round(progress * 100)}%` }} />
          </span>
          <span className="cw-row-left">{timeLeft(place)}</span>
        </span>
      ) : (
        <span className="cw-row-progress">
          <span className="cw-row-left dim">not started</span>
        </span>
      )}
      <span className="cw-row-when">{watchedWhen(item.lastWatched, now)}</span>
    </div>
  );
});
