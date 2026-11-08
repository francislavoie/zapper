// The Shows tab's episode pane: the keys between the list and the episodes,
// the seasons `[` and `]` step through, and what the pane and the sheet say
// of a show's seasons. Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import { paneKey, stepSeason } from "../logic/showPane.ts";
import { seasonsSummary, watchedSummary } from "../logic/shows.ts";
import { initialShows, showsReducer } from "../showsState.ts";
import { SHOW_FAVOURITES } from "../logic/shows.ts";

const list = { selected: true, posters: false };

test("Enter or → from the list goes into the episodes, opening the pane", () => {
  assert.deepEqual(paneKey("closed", "Enter", list), { zone: "episodes" });
  assert.deepEqual(paneKey("list", "Enter", list), { zone: "episodes" });
  assert.deepEqual(paneKey("closed", "ArrowRight", list), { zone: "episodes" });
  assert.deepEqual(paneKey("list", "ArrowRight", list), { zone: "episodes" });
  // In the poster grid → moves along the row.
  assert.equal(paneKey("list", "ArrowRight", { selected: true, posters: true }), null);
  // Nothing selected: no pane to go into.
  assert.equal(paneKey("closed", "Enter", { selected: false, posters: false }), null);
});

test("the list's own keys are left to it", () => {
  for (const key of ["ArrowUp", "ArrowDown", "PageDown", "Home", "End", "ArrowLeft", "a"]) {
    assert.equal(paneKey("list", key, list), null, key);
  }
});

test("in the episodes: Enter plays, [ ] change season, ← or Esc go back", () => {
  assert.deepEqual(paneKey("episodes", "Enter", list), { zone: "episodes", play: true });
  assert.deepEqual(paneKey("episodes", "[", list), { zone: "episodes", season: -1 });
  assert.deepEqual(paneKey("episodes", "]", list), { zone: "episodes", season: 1 });
  assert.deepEqual(paneKey("episodes", "ArrowLeft", list), { zone: "list" });
  assert.deepEqual(paneKey("episodes", "Escape", list), { zone: "list" });
  // ↑ ↓ are the episode list's own.
  assert.equal(paneKey("episodes", "ArrowDown", list), null);
  // → is taken and goes nowhere: it must not seek the player.
  assert.deepEqual(paneKey("episodes", "ArrowRight", list), { zone: "episodes" });
});

test("Esc from the episodes goes back to the list; from the list it is the Escape chain's", () => {
  assert.equal(paneKey("episodes", "Escape", list).zone, "list");
  // The chain closes the pane (logic/escape.ts, checked in back.test.mjs).
  assert.equal(paneKey("list", "Escape", list), null);
  assert.equal(paneKey("closed", "Escape", list), null);
});

test("[ and ] step through the show's seasons, held at either end", () => {
  const seasons = [1, 2, 3, 0];
  assert.equal(stepSeason(seasons, 2, 1), 3);
  assert.equal(stepSeason(seasons, 3, 1), 0);
  assert.equal(stepSeason(seasons, 0, 1), 0);
  assert.equal(stepSeason(seasons, 1, -1), 1);
  assert.equal(stepSeason(seasons, null, 1), 1);
  assert.equal(stepSeason([], 1, 1), null);
});

const season = (number, episodeCount, watchedCount) => ({ number, name: "", episodeCount, watchedCount });
const details = (seasons) => ({ seasons });

test("a show's seasons in a line: seasons, episodes, watched; specials apart", () => {
  assert.equal(seasonsSummary(details([season(1, 8, 2), season(2, 6, 0)])), "2 seasons · 14 episodes · 2 watched");
  assert.equal(seasonsSummary(details([season(1, 1, 0)])), "1 season · 1 episode");
  assert.equal(seasonsSummary(details([season(1, 8, 8), season(0, 3, 1)])), "1 season · 8 episodes · 8 watched · 3 specials");
  assert.equal(seasonsSummary(details([season(0, 2, 0)])), "2 specials");
  assert.equal(seasonsSummary(details([])), "");
  assert.equal(watchedSummary(details([season(1, 8, 2), season(2, 6, 0)])), "2 of 14 episodes watched");
  assert.equal(watchedSummary(details([season(1, 8, 0)])), "8 episodes, none watched yet");
  assert.equal(watchedSummary(details([])), "");
});

const show = (id) => ({
  seriesId: id, name: `Show ${id}`, cleanName: `Show ${id}`, year: 2020, rating: 7, posterUrl: null,
  added: null, categoryId: "s1", seasonCount: 2, episodeCount: 12, favourite: false, watched: false,
});

test("a click opens the pane, the arrows keep it as it was, Escape closes it", () => {
  let s = initialShows("list");
  assert.equal(s.paneOpen, false);
  s = showsReducer(s, { type: "shows/selectShow", show: show(1) });
  assert.equal(s.paneOpen, false, "an arrow key selects without opening");
  s = showsReducer(s, { type: "shows/selectShow", show: show(2), pane: true });
  assert.equal(s.paneOpen, true);
  s = showsReducer(s, { type: "shows/selectShow", show: show(3) });
  assert.equal(s.paneOpen, true, "an open pane follows the selection");
  s = showsReducer(s, { type: "shows/pane", open: false });
  assert.equal(s.paneOpen, false);
  assert.equal(s.selected?.seriesId, 3, "closing it keeps the selection, and the band with it");
});

test("the pane's season is per show, and a rail choice closes the pane", () => {
  let s = showsReducer(initialShows("list"), { type: "shows/selectShow", show: show(1), pane: true });
  s = showsReducer(s, { type: "shows/season", number: 2 });
  assert.equal(s.season, 2);
  s = showsReducer(s, { type: "shows/selectShow", show: show(1) });
  assert.equal(s.season, 2, "the same show keeps its season");
  s = showsReducer(s, { type: "shows/selectShow", show: show(2) });
  assert.equal(s.season, null, "another show opens on its own");
  s = showsReducer(s, { type: "shows/select", id: SHOW_FAVOURITES });
  assert.equal(s.paneOpen, false);
  assert.equal(s.selected, null);
});
