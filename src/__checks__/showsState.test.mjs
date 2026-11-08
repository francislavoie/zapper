// The Shows slice of the store: what choosing, marking and playing do to it.
// Run: node --test src/__checks__/

import assert from "node:assert/strict";
import { test } from "node:test";
import { SHOW_FAVOURITES, SHOW_RECENT } from "../logic/shows.ts";
import { initialShows, isShowsAction, showsReducer } from "../showsState.ts";

const show = (id) => ({
  seriesId: id, name: `Show ${id}`, cleanName: `Show ${id}`, year: 2020, rating: 7, posterUrl: null,
  added: null, categoryId: "s1", seasonCount: 2, episodeCount: 12, favourite: false, watched: false,
});
const episode = (id, seriesId = 1) => ({
  episodeId: id, seriesId, season: 1, episode: 1, episodeEnd: null, title: null, plot: null, durationSecs: 2400,
  stillUrl: null, airDate: null, watched: false, position: null, duration: null,
});
const marks = (id, o = {}) => ({ id, favourite: false, watched: false, position: null, duration: null, ...o });
const summary = (origin) => ({
  status: "ready",
  data: { available: true, note: null, total: 5, favourites: 0, recent: 5, watched: 0, origin },
});

test("starts on what was added recently, as a list", () => {
  const s = initialShows("list");
  assert.equal(s.selection, SHOW_RECENT);
  assert.equal(s.sort, "added");
  assert.equal(s.view, "list");
  assert.equal(s.playing, null);
});
test("choosing a rail item clears the search, the open show, its pane and the sheet", () => {
  let s = initialShows("list");
  s = showsReducer(s, { type: "shows/query", text: "harbour" });
  s = showsReducer(s, { type: "shows/selectShow", show: show(1), pane: true });
  s = showsReducer(s, { type: "shows/sheet", id: 1 });
  s = showsReducer(s, { type: "shows/season", number: 2 });
  s = showsReducer(s, { type: "shows/select", id: SHOW_FAVOURITES });
  assert.equal(s.selection, SHOW_FAVOURITES);
  assert.equal(s.query, "");
  assert.equal(s.selected, null);
  assert.equal(s.sheetId, null);
  assert.equal(s.season, null);
  assert.equal(s.paneOpen, false);
});
test("another show opens on its own season", () => {
  let s = showsReducer(initialShows("list"), { type: "shows/selectShow", show: show(1) });
  s = showsReducer(s, { type: "shows/season", number: 3 });
  assert.equal(s.season, 3);
  s = showsReducer(s, { type: "shows/selectShow", show: show(2) });
  assert.equal(s.season, null);
});
test("the playing episode is set, and cleared with null", () => {
  let s = showsReducer(initialShows("list"), { type: "shows/playing", playing: { series: show(1), episode: episode(7) } });
  assert.equal(s.playing.episode.episodeId, 7);
  s = showsReducer(s, { type: "shows/playing", playing: null });
  assert.equal(s.playing, null);
});
test("Cancel on Up next is kept for its episode, and a new open forgets it", () => {
  let s = initialShows("list");
  assert.equal(s.upNextCancelled, null);
  s = showsReducer(s, { type: "shows/upNextCancel", episodeId: 101 });
  assert.equal(s.upNextCancelled, 101);
  s = showsReducer(s, { type: "shows/playing", playing: { series: show(1), episode: episode(102) } });
  assert.equal(s.upNextCancelled, null);
});
test("an episode played or marked has its show's details and the lists read again", () => {
  let s = initialShows("list");
  const before = s.revision;
  s = showsReducer(s, { type: "shows/played", seriesId: 4 });
  assert.equal(s.detailsRevision[4], 1);
  assert.equal(s.revision, before + 1);
  s = showsReducer(s, { type: "shows/played", seriesId: 4 });
  assert.equal(s.detailsRevision[4], 2);
  assert.equal(s.detailsRevision[5], undefined);
});
test("a show's mark patches the open and playing show; an episode's, the playing episode", () => {
  let s = initialShows("list");
  s = showsReducer(s, { type: "shows/selectShow", show: show(1) });
  s = showsReducer(s, { type: "shows/playing", playing: { series: show(1), episode: episode(7) } });
  const before = s.revision;
  s = showsReducer(s, { type: "shows/marked", marks: marks(1, { favourite: true }) });
  assert.equal(s.selected.favourite, true);
  assert.equal(s.playing.series.favourite, true);
  assert.equal(s.seriesMarks[1].favourite, true);
  assert.equal(s.revision, before + 1);
  s = showsReducer(s, { type: "shows/episodeMarked", seriesId: 1, marks: marks(7, { watched: true }) });
  assert.equal(s.playing.episode.watched, true);
  assert.equal(s.episodeMarks[7].watched, true);
  assert.equal(s.seriesMarks[7], undefined, "a show's and an episode's marks are kept apart");
});
test("a new account resets the selection, the sheet and the marks", () => {
  let s = showsReducer(initialShows("list"), { type: "shows/summary", value: summary("a1") });
  s = showsReducer(s, { type: "shows/select", id: SHOW_FAVOURITES });
  s = showsReducer(s, { type: "shows/selectShow", show: show(1), pane: true });
  s = showsReducer(s, { type: "shows/sheet", id: 1 });
  s = showsReducer(s, { type: "shows/season", number: 2 });
  s = showsReducer(s, { type: "shows/marked", marks: marks(1, { favourite: true }) });
  s = showsReducer(s, { type: "shows/episodeMarked", seriesId: 1, marks: marks(7, { watched: true }) });
  const rev = s.revision;
  s = showsReducer(s, { type: "shows/summary", value: summary("b2") });
  assert.equal(s.summary.data.origin, "b2");
  assert.equal(s.selection, SHOW_RECENT);
  assert.equal(s.selected, null);
  assert.equal(s.sheetId, null);
  assert.equal(s.season, null);
  assert.equal(s.paneOpen, false);
  assert.deepEqual(s.seriesMarks, {});
  assert.deepEqual(s.episodeMarks, {});
  assert.equal(s.revision, rev + 1);
});
test("the same account's refreshed summary keeps what is open", () => {
  let s = showsReducer(initialShows("list"), { type: "shows/summary", value: summary("a1") });
  s = showsReducer(s, { type: "shows/sheet", id: 1 });
  s = showsReducer(s, { type: "shows/summary", value: summary("a1") });
  assert.equal(s.sheetId, 1);
});
test("a failed quiet summary refresh keeps the ready one", () => {
  const first = summary("a1");
  let s = showsReducer(initialShows("list"), { type: "shows/summary", value: first });
  s = showsReducer(s, { type: "shows/summary", value: { status: "loading" } });
  s = showsReducer(s, { type: "shows/summary", value: { status: "error", message: "offline" } });
  assert.equal(s.summary, first);
});
test("a sync reads the lists again with the service's marks", () => {
  let s = showsReducer(initialShows("list"), { type: "shows/marked", marks: marks(1) });
  const rev = s.revision;
  s = showsReducer(s, { type: "shows/catalogue" });
  assert.deepEqual(s.seriesMarks, {});
  assert.equal(s.revision, rev + 1);
});
test("choosing a rail item marks the selection as chosen", () => {
  let s = initialShows("list");
  assert.equal(s.chosen, false);
  s = showsReducer(s, { type: "shows/select", id: SHOW_FAVOURITES });
  assert.equal(s.chosen, true);
});
test("only shows/ actions are the shows reducer's", () => {
  assert.equal(isShowsAction({ type: "movies/query" }), false);
  assert.equal(isShowsAction({ type: "shows/query" }), true);
});
