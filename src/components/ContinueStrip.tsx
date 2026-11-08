// The strip above the posters: Continue watching as one row of wide cards,
// so what to pick up again is in sight whichever category is open. A card is
// a button (Tab reaches each one): it plays, asking first where the
// Resume question applies; the menu is Continue watching's own.

import { useMemo, type KeyboardEvent } from "react";
import { playContinue } from "../actions";
import { useClock } from "../hooks/useClock";
import { continueKey, stripCard, STRIP_CARD_WIDTH } from "../logic/continueWatching";
import { useApp, useDispatch } from "../store";
import type { ContinueItemDto } from "../types";
import { Poster } from "./Poster";

export function ContinueStrip({ tab }: { tab: "movies" | "shows" }) {
  const { continue: cw } = useApp();
  const dispatch = useDispatch();
  const now = useClock(60_000);
  const items: readonly ContinueItemDto[] = cw.items.status === "ready" ? cw.items.data : [];
  const cards = useMemo(() => items.map((i) => ({ item: i, card: stripCard(i, now) })), [items, now]);
  if (cards.length === 0) return null;

  const menu = (i: ContinueItemDto, x: number, y: number, takeFocus: boolean) =>
    dispatch({ type: "continue/menu", menu: { key: continueKey(i), x, y, takeFocus } });

  return (
    <section className="cw-strip" data-tab={tab} aria-label="Continue watching">
      <div className="cw-strip-head">
        <span className="cw-strip-title">Continue watching</span>
        <span className="cw-strip-sub">resume where you stopped</span>
      </div>
      <div className="cw-strip-row" style={{ ["--strip-card" as string]: `${STRIP_CARD_WIDTH}px` }}>
        {cards.map(({ item, card }) => (
          <button
            key={card.key}
            className="cw-strip-card"
            onClick={() => void playContinue(dispatch, item, "ask")}
            onContextMenu={(e) => {
              e.preventDefault();
              menu(item, e.clientX, e.clientY, false);
            }}
            onKeyDown={(e: KeyboardEvent<HTMLButtonElement>) => {
              if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
                e.preventDefault();
                const r = e.currentTarget.getBoundingClientRect();
                menu(item, r.left + 8, r.bottom - 8, true);
              }
            }}
          >
            <span className="cw-strip-art">
              <Poster url={card.art} name={card.title} className="poster cw-strip-img" />
              <span className="cw-strip-over">{card.title}</span>
              {card.progress !== null ? (
                <span className="poster-progress" aria-hidden>
                  <span style={{ width: `${Math.round(card.progress * 100)}%` }} />
                </span>
              ) : null}
            </span>
            <span className="cw-strip-line" data-tip-overflow={card.line}>
              {card.line}
            </span>
            <span className="cw-strip-foot">
              <span className="cw-strip-left">{card.left}</span>
              <span className="cw-strip-when">{card.when}</span>
            </span>
          </button>
        ))}
      </div>
    </section>
  );
}
