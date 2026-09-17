// Ctrl+P. One input, two kinds of result: the fixed action list filtered
// locally, and channels from the same nucleo index that serves `/`.

import { useCallback, useEffect, useRef, useState } from "react";
import { playChannel, runAction } from "../actions";
import { useSearch } from "../hooks/useSearch";
import {
  availableActions,
  clampIndex,
  filterActions,
  moveIndex,
  paletteItems,
  type PaletteItem,
} from "../logic/palette";
import { useApp, useDispatch } from "../store";

const PALETTE_CHANNEL_LIMIT = 50;

export function CommandPalette() {
  const state = useApp();
  const dispatch = useDispatch();
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const listRef = useRef<HTMLDivElement | null>(null);
  const results = useSearch(query, PALETTE_CHANNEL_LIMIT);

  const playing = state.player.state !== "stopped";
  const items = paletteItems(
    filterActions(availableActions(playing), query),
    results.status === "ready" ? results.data : [],
  );
  const active = clampIndex(index, items.length);

  // A fresh palette every time: leaving yesterday's query in it turns Ctrl+P
  // into a guessing game.
  useEffect(() => {
    setQuery("");
    setIndex(0);
  }, []);

  useEffect(() => {
    const el = listRef.current?.querySelector<HTMLElement>(".palette-item.active");
    el?.scrollIntoView({ block: "nearest" });
  }, [active, items.length]);

  const close = useCallback(() => dispatch({ type: "palette", open: false }), [dispatch]);

  const choose = useCallback(
    (item: PaletteItem | undefined) => {
      if (!item) return;
      close();
      if (item.kind === "channel") {
        void playChannel(dispatch, item.channel);
        return;
      }
      // Shared with the toolbar's hamburger, which draws this same list: one
      // behaviour per action, wherever it was picked from.
      runAction(item.action.id, { dispatch, player: state.player });
    },
    [close, dispatch, state.player],
  );

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) close();
      }}
    >
      <div className="palette" role="dialog" aria-label="Command palette">
        <input
          className="palette-input"
          autoFocus
          spellCheck={false}
          placeholder="Run a command, or find a channel…"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setIndex(0);
          }}
          onKeyDown={(e) => {
            // Escape is handled here so it closes the palette and nothing
            // else: one Escape, one dismissal.
            if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              close();
            } else if (e.key === "ArrowDown") {
              e.preventDefault();
              setIndex(moveIndex(active, 1, items.length));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setIndex(moveIndex(active, -1, items.length));
            } else if (e.key === "PageDown") {
              e.preventDefault();
              setIndex(moveIndex(active, 8, items.length));
            } else if (e.key === "PageUp") {
              e.preventDefault();
              setIndex(moveIndex(active, -8, items.length));
            } else if (e.key === "Enter") {
              e.preventDefault();
              choose(items[active]);
            }
          }}
        />

        <div className="palette-list" ref={listRef}>
          {items.map((item, i) => (
            <button
              key={item.kind === "action" ? `a-${item.action.id}` : `c-${item.channel.streamId}`}
              className={`palette-item ${i === active ? "active" : ""}`}
              onMouseMove={() => setIndex(i)}
              onClick={() => choose(item)}
            >
              <span className="palette-kind">
                {item.kind === "action" ? "action" : "channel"}
              </span>
              <span className="palette-label">
                {item.kind === "action" ? item.action.label : item.channel.name}
              </span>
              <span className="palette-hint">
                {item.kind === "action"
                  ? (item.action.hint ?? "")
                  : (item.channel.num ?? "")}
              </span>
            </button>
          ))}

          {results.status === "loading" ? (
            <div className="palette-empty">Searching…</div>
          ) : null}
          {results.status === "error" ? (
            <div className="palette-empty">Search failed — {results.message}</div>
          ) : null}
          {items.length === 0 && results.status !== "loading" ? (
            <div className="palette-empty">
              {query.trim() === "" ? "Type to search." : `Nothing matches “${query}”.`}
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
}
