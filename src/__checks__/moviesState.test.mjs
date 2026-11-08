// The Movies slice of the store: what choosing, marking and playing do to it.
// Run: node --test src/__checks__/

import assert from "node:assert/strict";
import { test } from "node:test";
import { MOVIE_FAVOURITES, MOVIE_RECENT } from "../logic/movies.ts";
import { initialMovies, isMoviesAction, moviesReducer } from "../moviesState.ts";

const movie = (id) => ({
  movieId: id, name: `Film ${id}`, cleanName: `Film ${id}`, year: 2020, rating: 7, posterUrl: null,
  added: null, durationSecs: 5400, categoryId: "c1", favourite: false, watched: false,
});
const marks = (id, o = {}) => ({ id, favourite: false, watched: false, position: null, duration: null, ...o });

test("starts on what was added recently, newest first", () => {
  const s = initialMovies("list");
  assert.equal(s.selection, MOVIE_RECENT);
  assert.equal(s.sort, "added");
  assert.equal(s.view, "list");
});
test("choosing a rail item clears the search, the open title and the sheet", () => {
  let s = initialMovies("list");
  s = moviesReducer(s, { type: "movies/query", text: "harbour" });
  s = moviesReducer(s, { type: "movies/selectMovie", movie: movie(1) });
  s = moviesReducer(s, { type: "movies/sheet", id: 1 });
  s = moviesReducer(s, { type: "movies/select", id: MOVIE_FAVOURITES });
  assert.equal(s.selection, MOVIE_FAVOURITES);
  assert.equal(s.query, "");
  assert.equal(s.selected, null);
  assert.equal(s.sheetId, null);
});
test("a mark patches the open and playing titles with that id, and refetches lists", () => {
  let s = initialMovies("list");
  s = moviesReducer(s, { type: "movies/selectMovie", movie: movie(1) });
  s = moviesReducer(s, { type: "movies/playing", movie: movie(1) });
  const before = s.revision;
  s = moviesReducer(s, { type: "movies/marked", marks: marks(1, { favourite: true, watched: true }) });
  assert.equal(s.selected.favourite, true);
  assert.equal(s.playing.watched, true);
  assert.equal(s.revision, before + 1);
  assert.equal(s.marks[1].favourite, true);
});
test("a mark for another title leaves the open one alone", () => {
  let s = moviesReducer(initialMovies("list"), { type: "movies/selectMovie", movie: movie(1) });
  const open = s.selected;
  s = moviesReducer(s, { type: "movies/marked", marks: marks(2, { favourite: true }) });
  assert.equal(s.selected, open);
});
test("a header menu closes when it is used", () => {
  let s = moviesReducer(initialMovies("list"), { type: "movies/menu", menu: { kind: "sort", x: 1, y: 2 } });
  assert.equal(s.menu.kind, "sort");
  s = moviesReducer(s, { type: "movies/sort", sort: "az" });
  assert.equal(s.menu, null);
  assert.equal(s.sort, "az");
  s = moviesReducer(s, { type: "movies/menu", menu: { kind: "years", x: 1, y: 2 } });
  s = moviesReducer(s, { type: "movies/years", range: { from: 1990, to: 1999 } });
  assert.equal(s.menu, null);
});
test("only movies/ actions are the movies reducer's", () => {
  assert.equal(isMoviesAction({ type: "query" }), false);
  assert.equal(isMoviesAction({ type: "movies/query" }), true);
});

test("a summary that was ready stays ready through a failed or repeated refresh", () => {
  const ready = { status: "ready", data: { available: true, note: null, total: 5, favourites: 0, recent: 5, watched: 0, origin: "a1" } };
  let s = moviesReducer(initialMovies("list"), { type: "movies/summary", value: ready });
  s = moviesReducer(s, { type: "movies/summary", value: { status: "loading" } });
  s = moviesReducer(s, { type: "movies/summary", value: { status: "error", message: "offline" } });
  assert.equal(s.summary, ready);
  // A newer answer still replaces it.
  const gone = { status: "ready", data: { ...ready.data, available: false } };
  s = moviesReducer(s, { type: "movies/summary", value: gone });
  assert.equal(s.summary, gone);
});

const summary = (origin) => ({
  status: "ready",
  data: { available: true, note: null, total: 5, favourites: 0, recent: 5, watched: 0, origin },
});
const browsing = () => {
  let s = moviesReducer(initialMovies("list"), { type: "movies/summary", value: summary("a1") });
  s = moviesReducer(s, { type: "movies/select", id: "c7" });
  s = moviesReducer(s, { type: "movies/selectMovie", movie: movie(1) });
  s = moviesReducer(s, { type: "movies/sheet", id: 1 });
  s = moviesReducer(s, { type: "movies/marked", marks: marks(1, { favourite: true }) });
  return s;
};
test("another account's movies leave nothing of the old ones in view", () => {
  const before = browsing();
  const s = moviesReducer(before, { type: "movies/summary", value: summary("b2") });
  assert.equal(s.summary.data.origin, "b2");
  assert.deepEqual(s.marks, {});
  assert.equal(s.selected, null);
  assert.equal(s.sheetId, null);
  assert.equal(s.selection, MOVIE_RECENT);
  assert.equal(s.revision, before.revision + 1);
});
test("the same account's refreshed summary keeps what is open", () => {
  const before = browsing();
  const s = moviesReducer(before, { type: "movies/summary", value: summary("a1") });
  assert.equal(s.selected, before.selected);
  assert.equal(s.selection, "c7");
  assert.equal(s.marks[1].favourite, true);
  assert.equal(s.revision, before.revision);
});
test("the first summary is no change of account", () => {
  let s = moviesReducer(initialMovies("list"), { type: "movies/selectMovie", movie: movie(1) });
  s = moviesReducer(s, { type: "movies/summary", value: { status: "loading" } });
  s = moviesReducer(s, { type: "movies/summary", value: summary("a1") });
  assert.equal(s.selected.movieId, 1);
});
test("a catalogue refresh forgets the marks kept this session and reads the lists again", () => {
  const before = browsing();
  const s = moviesReducer(before, { type: "movies/catalogue" });
  assert.deepEqual(s.marks, {});
  assert.equal(s.revision, before.revision + 1);
  assert.equal(s.selected, before.selected, "the open title stays open");
});
test("choosing a rail item marks the selection as chosen", () => {
  let s = initialMovies("list");
  assert.equal(s.chosen, false);
  s = moviesReducer(s, { type: "movies/select", id: MOVIE_FAVOURITES });
  assert.equal(s.chosen, true);
});
test("a mark carries the place into the open and playing titles", () => {
  let s = initialMovies("list");
  s = moviesReducer(s, { type: "movies/selectMovie", movie: movie(1) });
  s = moviesReducer(s, { type: "movies/playing", movie: movie(1) });
  s = moviesReducer(s, {
    type: "movies/marked",
    marks: marks(1, { favourite: false, watched: false, position: 40, duration: 100, playedAt: 5 }),
  });
  assert.equal(s.selected.position, 40);
  assert.equal(s.selected.duration, 100);
  assert.equal(s.selected.playedAt, 5);
  assert.equal(s.playing.position, 40);
});
