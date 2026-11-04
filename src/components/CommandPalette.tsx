// Ctrl+P. One input, three kinds of result: the fixed action list filtered
// locally, channels from the same nucleo index that serves `/`, and
// programmes from the guide's title index, as the list's Programmes tab has.

import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { openProgrammeHit, playChannel, runAction } from "../actions";
import { capabilities } from "../backend";
import { moviesAvailable } from "../logic/movies";
import { useClock } from "../hooks/useClock";
import { useProgrammeSearch } from "../hooks/useProgrammeSearch";
import { useSearch } from "../hooks/useSearch";
import {
  availableActions,
  clampIndex,
  filterActions,
  groupHeading,
  moveIndex,
  paletteItems,
  type PaletteItem,
} from "../logic/palette";
import { hintLabel } from "../logic/keys";
import { airingTag, airingWhen } from "../logic/programmes";
import { IS_MAC } from "../platform";
import { categoryNames, categoryOf } from "../logic/searchGroups";
import { listedFor } from "../logic/railItems";
import { useApp, useDispatch } from "../store";
import { usePlayerRecord } from "../hooks/usePlayerRecord";

/** Enough to pick from without the programmes, which follow the channels,
 *  sitting a long scroll down. The list's own search shows more of both. */
const PALETTE_CHANNEL_LIMIT = 20;
const PALETTE_PROGRAMME_LIMIT = 20;

export function CommandPalette() {
  const state = useApp();
  const dispatch = useDispatch();
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const listRef = useRef<HTMLDivElement | null>(null);
  const results = useSearch(query, PALETTE_CHANNEL_LIMIT);
  const programmes = useProgrammeSearch(query, PALETTE_PROGRAMME_LIMIT).result;
  const now = useClock(15_000);
  const names = useMemo(
    () => categoryNames(state.categories.status === "ready" ? state.categories.data : []),
    [state.categories],
  );

  const playing = state.player.state !== "stopped";
  const record = usePlayerRecord();
  const items = paletteItems(
    filterActions(
      availableActions(playing, {
        capabilities,
        hasPrevious: state.previousChannel !== null,
        movies: moviesAvailable(state.movies.summary),
        sleeping: state.sleep !== null,
        programme: state.sleepProgramme !== null,
        recordable: record?.kind === "record",
      }),
      query,
    ),
    results.status === "ready" ? results.data : [],
    programmes.status === "ready" ? programmes.data : [],
  );
  const searching = results.status === "loading" || programmes.status === "loading";
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
      if (item.kind === "programme") {
        void openProgrammeHit(dispatch, item.hit, {
          categoryId: state.categoryId,
          mode: state.mode,
          guideHours: state.settings.guideHours,
          listed: listedFor(
            state.categoryId,
            state.favourites.status === "ready" ? state.favourites.data : null,
            state.recent.status === "ready" ? state.recent.data : null,
          ),
        });
        return;
      }
      // Shared with the toolbar's hamburger, which draws this same list: one
      // behaviour per action, wherever it was picked from.
      runAction(item.action.id, {
        dispatch,
        player: state.player,
        previous: state.previousChannel,
        record,
      });
    },
    [
      close,
      dispatch,
      record,
      state.player,
      state.previousChannel,
      state.categoryId,
      state.mode,
      state.settings.guideHours,
    ],
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
          placeholder="Run a command, or find a channel or programme  ·  a|b for either"
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
          {items.map((item, i) => {
            const heading = groupHeading(items, i);
            return (
              <Fragment key={itemKey(item)}>
                {heading ? <div className="palette-group">{heading}</div> : null}
                <button
                  className={`palette-item ${i === active ? "active" : ""}`}
                  onMouseMove={() => setIndex(i)}
                  onClick={() => choose(item)}
                >
                  <span className={`palette-kind ${item.kind === "programme" ? item.hit.airing : ""}`}>
                    {item.kind === "programme" ? airingTag(item.hit.airing) : item.kind}
                  </span>
                  <span className="palette-label" data-tip-overflow="">
                    {item.kind === "action"
                      ? item.action.label
                      : item.kind === "channel"
                        ? item.channel.name
                        : item.hit.programme.title}
                  </span>
                  {/* A label per row rather than headings: the palette is
                      already grouped by kind, and a heading per category
                      would split twenty results into twenty groups. */}
                  {item.kind === "channel" ? (
                    <span className="palette-where" data-tip-overflow="">
                      {categoryOf(names, item.channel.categoryId)}
                    </span>
                  ) : item.kind === "programme" ? (
                    <span className="palette-where" data-tip-overflow="">
                      {item.hit.channel.cleanName} · {categoryOf(names, item.hit.channel.categoryId)}
                    </span>
                  ) : null}
                  <span className="palette-hint">
                    {item.kind === "action"
                      ? hintLabel(item.action.hint ?? "", IS_MAC)
                      : item.kind === "channel"
                        ? (item.channel.numLabel ?? item.channel.num ?? "")
                        : airingWhen(item.hit, now)}
                  </span>
                </button>
              </Fragment>
            );
          })}

          {searching ? <div className="palette-empty">Searching…</div> : null}
          {results.status === "error" ? (
            <div className="palette-empty">Search failed — {results.message}</div>
          ) : null}
          {programmes.status === "error" ? (
            <div className="palette-empty">Programme search failed — {programmes.message}</div>
          ) : null}
          {items.length === 0 && !searching ? (
            <div className="palette-empty">
              {query.trim() === "" ? "Type to search." : `Nothing matches “${query}”.`}
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
}

function itemKey(item: PaletteItem): string {
  if (item.kind === "action") return `a-${item.action.id}`;
  if (item.kind === "channel") return `c-${item.channel.streamId}`;
  return `p-${item.hit.channel.streamId}-${item.hit.programme.start}`;
}
