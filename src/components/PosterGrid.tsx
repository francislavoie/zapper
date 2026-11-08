// An on-demand list as posters: a grid of cards, windowed by rows the way the
// list is windowed by titles, so only what is on screen (and a little either
// side) exists as DOM. The scrolling element and the geometry belong to
// OnDemandList, which also needs them for the keyboard and for asking the
// right pages to be fetched; this draws the visible cards where it is told.
// What a card says comes from `card`, so the grid works for any kind of title.

import { memo } from "react";
import { POSTER_GAP } from "../logic/movies";
import type { PosterCard, posterGrid } from "../logic/movies";
import type { MenuAt, RowKey } from "./OnDemandList";
import { Poster } from "./Poster";

export type PosterGeometry = ReturnType<typeof posterGrid>;

export interface PosterGridProps<T> {
  /** Which rows are drawn, and the column and card sizes. */
  geometry: PosterGeometry;
  /** The first and one past the last row to draw. */
  rows: { start: number; end: number };
  total: number;
  rowAt: (index: number) => T | undefined;
  /** The title's id: the card's key, its element id and the selection. */
  idOf: (t: T) => RowKey;
  /** What the card draws. Called inside the memoised card, so it should be
   *  the same function from one render to the next. */
  card: (t: T, now: number) => PosterCard;
  selectedId: RowKey | null;
  now: number;
  onSelect: (t: T, index: number) => void;
  onOpen: (t: T, index: number) => void;
  /** A right-click on a card, where the list has a menu for its titles. */
  onMenu?: (t: T, index: number, at: MenuAt) => void;
  idPrefix: string;
}

export function PosterGrid<T>({
  geometry,
  rows,
  total,
  rowAt,
  idOf,
  card,
  selectedId,
  now,
  onSelect,
  onOpen,
  onMenu,
  idPrefix,
}: PosterGridProps<T>) {
  const { columns, cardWidth, rowHeight } = geometry;
  const cards = [];
  for (let r = rows.start; r < rows.end; r++) {
    for (let c = 0; c < columns; c++) {
      const i = r * columns + c;
      if (i >= total) break;
      const t = rowAt(i);
      const left = POSTER_GAP + c * (cardWidth + POSTER_GAP);
      const top = POSTER_GAP + r * rowHeight;
      if (t === undefined) {
        cards.push(<div key={`gap-${i}`} className="poster-card gap" style={{ left, top, width: cardWidth }} />);
        continue;
      }
      const id = idOf(t);
      cards.push(
        <Card
          key={id}
          id={id}
          item={t}
          card={card as CardOf}
          index={i}
          idPrefix={idPrefix}
          left={left}
          top={top}
          width={cardWidth}
          selected={id === selectedId}
          now={now}
          onSelect={onSelect as CardProps["onSelect"]}
          onOpen={onOpen as CardProps["onOpen"]}
          onMenu={onMenu as CardProps["onMenu"]}
        />,
      );
    }
  }
  return <>{cards}</>;
}

/** The card is memoised once for every kind; its item is opaque to it. */
type CardOf = (t: unknown, now: number) => PosterCard;

interface CardProps {
  /** `idOf(item)`: the element's id, so the list's active descendant (made
   *  from the same function) always names a card that is there. */
  id: RowKey;
  item: unknown;
  card: CardOf;
  index: number;
  idPrefix: string;
  left: number;
  top: number;
  width: number;
  selected: boolean;
  now: number;
  onSelect: (t: unknown, index: number) => void;
  onOpen: (t: unknown, index: number) => void;
  onMenu?: (t: unknown, index: number, at: MenuAt) => void;
}

const Card = memo(function Card({
  id,
  item,
  card,
  index,
  idPrefix,
  left,
  top,
  width,
  selected,
  now,
  onSelect,
  onOpen,
  onMenu,
}: CardProps) {
  const c = card(item, now);
  return (
    <div
      className={`poster-card ${selected ? "sel" : ""}`}
      id={`${idPrefix}-${id}`}
      role="option"
      aria-selected={selected}
      style={{ left, top, width }}
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
      <div className="poster-art" style={{ height: Math.round(width * 1.5) }}>
        <Poster url={c.posterUrl} name={c.title} className="poster poster-img" />
        {c.badges.length > 0 ? (
          <span className="poster-badges">
            {c.badges.map((b) => (
              <span className="badge movie-badge" key={b}>
                {b}
              </span>
            ))}
          </span>
        ) : null}
        {c.watched ? (
          <span className="poster-watched" data-tip="Watched" aria-label="Watched">
            ✓
          </span>
        ) : null}
        <span className="poster-title" data-tip-overflow={c.name}>
          {c.title}
        </span>
      </div>
      <span className="poster-meta">{c.meta}</span>
    </div>
  );
});
