// What Continue watching does across the store's slices. Run: node --test src/__checks__/

import assert from "node:assert/strict";
import { test } from "node:test";
import { reduceSlices, syncedSlices } from "../continueRoot.ts";
import { initialContinue } from "../continueState.ts";
import { CONTINUE } from "../logic/continueWatching.ts";
import { MOVIE_FAVOURITES, MOVIE_RECENT } from "../logic/movies.ts";
import { SHOW_RECENT } from "../logic/shows.ts";
import { initialMovies } from "../moviesState.ts";
import { initialShows } from "../showsState.ts";

const start = () => ({
  movies: initialMovies("list"),
  shows: initialShows("list"),
  continue: initialContinue(),
});
const entry = (id) => ({
  kind: "movie",
  lastWatched: 1,
  movie: { movieId: id, cleanName: `Film ${id}`, favourite: false, watched: false, position: 60, duration: 600, durationSecs: 600, playedAt: 1 },
});
const arrive = (s, n = 2, revision) =>
  reduceSlices(s, {
    type: "continue/items",
    value: { status: "ready", data: Array.from({ length: n }, (_, i) => entry(i + 1)) },
    revision,
  });
const moviesSummary = (origin) => ({ status: "ready", data: { available: true, origin } });
const showsSummary = (origin) => ({ status: "ready", data: { available: true, origin } });

test("the first list opens both tabs on Continue watching", () => {
  const s = arrive(start());
  assert.equal(s.movies.selection, CONTINUE);
  assert.equal(s.shows.selection, CONTINUE);
  assert.equal(s.movies.chosen, false);
});
test("an empty list leaves the tabs on Recently added", () => {
  const s = arrive(start(), 0);
  assert.equal(s.movies.selection, MOVIE_RECENT);
  assert.equal(s.shows.selection, SHOW_RECENT);
});
test("a chosen selection is not taken over", () => {
  let s = reduceSlices(start(), { type: "movies/select", id: MOVIE_RECENT });
  s = arrive(s);
  assert.equal(s.movies.selection, MOVIE_RECENT);
  assert.equal(s.shows.selection, CONTINUE);
});
test("a title opened from the default selection is not pulled away by a refresh", () => {
  let s = reduceSlices(start(), { type: "movies/selectMovie", movie: { movieId: 9 } });
  s = arrive(s);
  assert.equal(s.movies.selection, MOVIE_RECENT);
  assert.equal(s.movies.selected.movieId, 9);
});
test("a typed search is not pulled away by a refresh", () => {
  let s = reduceSlices(start(), { type: "movies/query", text: "harb" });
  s = arrive(s);
  assert.equal(s.movies.selection, MOVIE_RECENT);
  assert.equal(s.movies.query, "harb");
});
test("a later refresh does not take back a tab the owner moved on from", () => {
  let s = arrive(start());
  s = reduceSlices(s, { type: "movies/select", id: MOVIE_FAVOURITES });
  s = arrive(s, 3);
  assert.equal(s.movies.selection, MOVIE_FAVOURITES);
});
test("another account's movies reset Continue watching and what was chosen", () => {
  let s = reduceSlices(start(), { type: "movies/summary", value: moviesSummary("a") });
  s = reduceSlices(s, { type: "movies/select", id: MOVIE_FAVOURITES });
  s = arrive(s);
  const rev = s.continue.revision;
  s = reduceSlices(s, { type: "movies/summary", value: moviesSummary("b") });
  assert.equal(s.continue.items.status, "idle");
  assert.equal(s.continue.revision, rev + 1);
  assert.equal(s.movies.chosen, false);
  assert.equal(s.movies.selection, MOVIE_RECENT);
});
test("another account's shows reset Continue watching and what was chosen", () => {
  let s = reduceSlices(start(), { type: "shows/summary", value: showsSummary("a") });
  s = arrive(s);
  s = reduceSlices(s, { type: "shows/select", id: SHOW_RECENT });
  const rev = s.continue.revision;
  s = reduceSlices(s, { type: "shows/summary", value: showsSummary("b") });
  assert.equal(s.continue.items.status, "idle");
  assert.equal(s.continue.revision, rev + 1);
  assert.equal(s.shows.chosen, false);
});
test("the same account's refreshed summary leaves Continue watching alone", () => {
  let s = reduceSlices(start(), { type: "movies/summary", value: moviesSummary("a") });
  s = arrive(s);
  const before = s.continue;
  s = reduceSlices(s, { type: "movies/summary", value: moviesSummary("a") });
  assert.equal(s.continue, before);
});
test("a sync that changed the catalogue sends continue/changed", () => {
  const s = start();
  const after = syncedSlices(s);
  assert.equal(after.continue.revision, s.continue.revision + 1);
  assert.equal(after.movies.revision, s.movies.revision + 1);
  assert.equal(after.shows.revision, s.shows.revision + 1);
});
test("a list that began before another account's reset is dropped when it lands", () => {
  let s = arrive(start(), 2, 0);
  s = reduceSlices(s, { type: "continue/reset" });
  const stale = reduceSlices(s, {
    type: "continue/items",
    value: { status: "ready", data: [entry(7)] },
    revision: 0,
  });
  assert.equal(stale.continue.items.status, "idle");
  const fresh = arrive(s, 1, s.continue.revision);
  assert.equal(fresh.continue.items.status, "ready");
});
