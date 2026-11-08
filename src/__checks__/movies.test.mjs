// What the Movies pane says about a title: titles, facts, badges, the poster
// grid's arithmetic. Pure; the logic lives in src/logic/movies.ts.
// Run: node --test src/__checks__/

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  MOVIE_FAVOURITES,
  MOVIE_RECENT,
  MOVIE_WATCHED,
  POSTER_MIN_WIDTH,
  fixedMovieItems,
  languagesLine,
  moviesAvailable,
  movieBadges,
  movieCard,
  movieCount,
  movieFacts,
  movieQuery,
  movieRailKind,
  movieTitle,
  parseMoviesView,
  peopleLine,
  playingKind,
  posterGrid,
  recallDetails,
  rememberDetails,
  runtime,
  yearRanges,
} from "../logic/movies.ts";

test("the rail's own movie items, counted in words", () => {
  const f = fixedMovieItems({ available: true, note: null, total: 159000, favourites: 12, recent: 240, watched: 0, origin: "a1" });
  assert.deepEqual(f.map((x) => [x.id, x.name, x.count]), [[MOVIE_FAVOURITES, "Favourites", "12"], [MOVIE_RECENT, "Recently added", "240"], [MOVIE_WATCHED, "Watched", ""]]);
  assert.deepEqual(fixedMovieItems(null).map((x) => x.count), ["", "", ""]);
  const s = { available: true, note: null, total: 159000, favourites: 12, recent: 240, watched: 0, origin: "a1" };
  assert.deepEqual(fixedMovieItems(s, 4)[0], { id: "*continue*", name: "Continue watching", count: "4" });
  assert.equal(fixedMovieItems(s, 0)[0].count, "");
  assert.equal(fixedMovieItems(s).some((x) => x.id === "*continue*"), false);
  assert.equal(fixedMovieItems(s, null).some((x) => x.id === "*continue*"), false);
});
test("a selection becomes the query the backend reads", () => {
  assert.deepEqual(movieQuery("c7", "az", { from: 1990, to: 1999 }), { list: "category", categoryId: "c7", sort: "az", yearFrom: 1990, yearTo: 1999, search: null });
  assert.deepEqual(movieQuery(MOVIE_RECENT, "added", null), { list: "recent", categoryId: null, sort: "added", yearFrom: null, yearTo: null, search: null });
  assert.equal(movieQuery(MOVIE_WATCHED, "az", null).list, "watched");
});
test("a search is every movie that matches, in the list's sort and years", () => {
  assert.deepEqual(movieQuery(MOVIE_RECENT, "added", { from: 2020, to: 2029 }, "  EN - "), {
    list: "all",
    categoryId: null,
    sort: "added",
    yearFrom: 2020,
    yearTo: 2029,
    search: "EN -",
  });
  assert.equal(movieQuery("c7", "az", null, "   ").list, "category", "blank is no search");
  assert.equal(movieQuery("c7", "az", null, " | ").list, "category", "only pipes is no search");
  assert.equal(movieQuery("c7", "az", null, "|").search, null);
  // The query is the list's key: another search, or another sort of the
  // same one, is another list (and starts at the top); the rail's choice
  // under a search is not.
  const key = (selection, sort, search) => JSON.stringify(movieQuery(selection, sort, null, search));
  assert.notEqual(key("c7", "added", "EN -"), key("c7", "added", "FR -"));
  assert.notEqual(key("c7", "added", "EN -"), key("c7", "year", "EN -"));
  assert.notEqual(key("c7", "added", "EN -"), key("c7", "added", ""));
  assert.equal(key("c7", "added", "EN -"), key(MOVIE_RECENT, "added", " EN - "));
});
test("what a selection is called and how many it holds", () => {
  const cats = [{ id: "c7", name: "Harbour Tales" }];
  assert.equal(movieTitle("c7", cats), "Harbour Tales");
  assert.equal(movieTitle("gone", cats), "Movies");
  assert.equal(movieTitle(MOVIE_FAVOURITES, cats), "Favourites");
  assert.equal(movieRailKind("c7"), "category");
  assert.equal(movieRailKind(MOVIE_WATCHED), "watched");
  assert.equal(movieCount(1), "1 movie");
  assert.equal(movieCount(3812), "3,812 movies");
});
test("runtimes and facts read as the mockups do", () => {
  assert.equal(runtime(52 * 60), "52m");
  assert.equal(runtime(121 * 60), "2h 01m");
  assert.equal(runtime(null), "");
  assert.equal(runtime(0), "");
  assert.equal(movieFacts({ year: 2025, durationSecs: 109 * 60, rating: 7.6 }), "2025 · 1h 49m · ★ 7.6");
  assert.equal(movieFacts({ year: null, durationSecs: null, rating: 7 }), "★ 7.0");
  assert.equal(movieFacts({ year: null, durationSecs: null, rating: null }), "");
});
test("a movie as a poster card: the clean name drawn, the full name for its tip", () => {
  const now = 1_800_000_000;
  const m = {
    movieId: 3, name: "Film 4K", cleanName: "Film", year: 2025, durationSecs: 109 * 60, rating: 7.6,
    posterUrl: null, added: now - 86400, categoryId: "c1", favourite: true, watched: false,
  };
  assert.deepEqual(movieCard(m, now), {
    id: 3, title: "Film", name: "Film 4K", posterUrl: null, badges: ["NEW", "4K"],
    meta: "2025 · 1h 49m · ★ 7.6", watched: false, favourite: true, progress: null,
  });
  assert.equal(movieCard({ ...m, position: 30 * 60, duration: 100 * 60 }, now).progress, 0.3);
  assert.equal(movieCard({ ...m, year: null, durationSecs: null, rating: null, added: null, name: "Film" }, now).meta, "");
  assert.deepEqual(movieCard({ ...m, added: null, name: "Film" }, now).badges, []);
});
test("year ranges: all, the last two years, decades, before 1980", () => {
  const r = yearRanges(2026);
  assert.deepEqual(r.map((x) => x.label), ["All years", "2025–2026", "2020s", "2010s", "2000s", "1990s", "1980s", "Before 1980"]);
  assert.equal(r[0].range, null);
  assert.deepEqual(r[3].range, { from: 2010, to: 2019 });
  assert.deepEqual(r[7].range, { from: null, to: 1979 });
});
test("badges: new for a week, 4K when the name says so", () => {
  const now = 1_800_000_000;
  assert.deepEqual(movieBadges({ name: "Film 4K", added: now - 3 * 86400 }, now), ["NEW", "4K"]);
  assert.deepEqual(movieBadges({ name: "Film", added: now - 8 * 86400 }, now), []);
  assert.deepEqual(movieBadges({ name: "Film [UHD]", added: null }, now), ["4K"]);
  assert.deepEqual(movieBadges({ name: "Film 4000", added: null }, now), []);
});
test("the view is a list unless posters were chosen here", () => {
  assert.equal(parseMoviesView(null), "list");
  assert.equal(parseMoviesView("posters"), "posters");
  assert.equal(parseMoviesView("grid"), "list");
});
test("posters fill the width in whole columns", () => {
  const g = posterGrid(1000, 25);
  assert.equal(g.columns, 6);
  assert.equal(g.rows, 5);
  assert.ok(g.cardWidth >= POSTER_MIN_WIDTH);
  assert.equal(posterGrid(100, 3).columns, 1);
  assert.equal(posterGrid(1000, 0).rows, 0);
});
test("what is playing: a channel, a movie, an episode, or nothing", () => {
  assert.equal(playingKind({ state: "playing", streamId: 7, onDemand: null }), "channel");
  assert.equal(playingKind({ state: "paused", streamId: null, onDemand: { kind: "movie", id: 7, position: 1, duration: null, bufferedTo: null } }), "movie");
  assert.equal(playingKind({ state: "stopped", streamId: null, onDemand: null }), null);
  assert.equal(playingKind({ state: "playing", streamId: null, onDemand: { kind: "episode", id: 7, position: 1, duration: null, bufferedTo: null } }), "episode");
});
test("people and languages lines", () => {
  assert.equal(peopleLine({ director: "D", cast: ["A", "B", "C", "E", "F"] }), "Director D · With A, B, C, E");
  assert.equal(peopleLine({ director: null, cast: [] }), "");
  assert.equal(languagesLine({ audioLanguages: ["EN", "FR"], subtitleLanguages: ["EN", "FR"] }), "EN · FR audio · subtitles EN, FR");
  assert.equal(languagesLine({ audioLanguages: [], subtitleLanguages: ["EN"] }), "subtitles EN");
  assert.equal(languagesLine({ audioLanguages: [], subtitleLanguages: [] }), "");
});

test("the Movies tab shows only once the summary says the account has movies", () => {
  const data = (available) => ({ available, note: null, total: 1, favourites: 0, recent: 1, watched: 0, origin: "a1" });
  assert.equal(moviesAvailable({ status: "idle" }), false);
  assert.equal(moviesAvailable({ status: "loading" }), false);
  assert.equal(moviesAvailable({ status: "error", message: "x" }), false);
  assert.equal(moviesAvailable({ status: "ready", data: data(false) }), false);
  assert.equal(moviesAvailable({ status: "ready", data: data(true) }), true);
});

test("details are remembered by id, the oldest forgotten first", () => {
  const d = (id) => ({ movie: { movieId: id }, plot: null, genres: [], director: null, cast: [], releaseDate: null, backdropUrls: [], tmdbId: null, audioLanguages: [], subtitleLanguages: [], fetchedAt: null });
  const one = d(1);
  assert.equal(recallDetails(rememberDetails(new Map(), 1, one, 2), 1), one);
  assert.equal(recallDetails(new Map(), 9), undefined);
  const c = rememberDetails(rememberDetails(rememberDetails(new Map(), 1, d(1), 2), 2, d(2), 2), 3, d(3), 2);
  assert.equal(recallDetails(c, 1), undefined);
  assert.deepEqual([...c.keys()], [2, 3]);
});
test("recalling details refreshes their age", () => {
  const d = (id) => ({ movie: { movieId: id } });
  const c = new Map();
  rememberDetails(c, 1, d(1), 2);
  rememberDetails(c, 2, d(2), 2);
  assert.ok(recallDetails(c, 1));
  rememberDetails(c, 3, d(3), 2);
  assert.ok(recallDetails(c, 1), "the one just recalled stays");
  assert.equal(recallDetails(c, 2), undefined, "the one left alone goes");
});
