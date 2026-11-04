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
test("only movies/ actions are the movies reducer's", () => {
  assert.equal(isMoviesAction({ type: "query" }), false);
  assert.equal(isMoviesAction({ type: "movies/query" }), true);
});
