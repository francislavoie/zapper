// Continue watching: what an entry is called, drawn as and keyed by.
// Run: node --test src/__checks__/

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  CONTINUE,
  continueCard,
  continueCode,
  continueCount,
  continueShown,
  continueKey,
  continuePlace,
  continueTarget,
  continueTitle,
  firstSelection,
  freshOnReturn,
  stripCard,
} from "../logic/continueWatching.ts";

const now = new Date(2026, 10, 8, 12).getTime() / 1000;
const DAY = 86_400;
const movie = (id, o = {}) => ({
  movieId: id, name: "A Film 4K", cleanName: "A Film", year: 2025, durationSecs: 6000, rating: null,
  posterUrl: "/api/poster/m", added: null, categoryId: null, favourite: false, watched: false,
  position: 3000, duration: 6000, playedAt: now - 3600, ...o,
});
const show = (id) => ({
  seriesId: id, name: "A Show HD", cleanName: "A Show", year: 2020, rating: null, posterUrl: "/api/poster/s",
  added: null, categoryId: null, seasonCount: 2, episodeCount: 10, favourite: false, watched: false,
});
const episode = (id, seriesId, season, n, o = {}) => ({
  episodeId: id, seriesId, season, episode: n, episodeEnd: null, title: "Night Shift", plot: null,
  durationSecs: 3000, stillUrl: "/api/still/e", airDate: null, watched: false, position: 2100, duration: 3000, ...o,
});
const inProgress = { kind: "episode", series: show(12), episode: episode(7, 12, 2, 4), upNext: false, lastWatched: now - DAY };
const upNext = {
  kind: "episode", series: show(12), episode: episode(9, 12, 1, 3, { position: null, duration: null }),
  upNext: true, lastWatched: now - 5 * DAY,
};
const film = { kind: "movie", movie: movie(7), lastWatched: now - 3600 };

test("one entry per movie and per show", () => {
  assert.equal(continueKey(film), "movie:7");
  assert.equal(continueKey({ ...inProgress, series: show(7), episode: episode(1, 7, 1, 1) }), "show:7");
  assert.equal(continueKey({ ...inProgress, series: show(7), episode: episode(2, 7, 1, 2) }), "show:7");
  assert.deepEqual(continueTarget(inProgress), { kind: "series", id: 12 });
  assert.deepEqual(continueTarget(film), { kind: "movie", id: 7 });
});

test("an entry's name, code and place", () => {
  assert.equal(continueTitle(film), "A Film");
  assert.equal(continueTitle(inProgress), "A Show");
  assert.equal(continueCode(film), "");
  assert.equal(continueCode(inProgress), "S2 E4");
  assert.equal(continueCode(upNext), "S1 E3 · next");
  assert.deepEqual(continuePlace(film), { watched: false, position: 3000, duration: 6000, durationSecs: 6000 });
  assert.equal(continuePlace(inProgress).position, 2100);
});

test("an entry as a poster card", () => {
  assert.deepEqual(continueCard(film, now), {
    id: "movie:7", title: "A Film", name: "A Film 4K", posterUrl: "/api/poster/m", badges: [],
    meta: "50m left", watched: false, favourite: false, progress: 0.5,
  });
  const c = continueCard(inProgress, now);
  assert.equal(c.posterUrl, "/api/poster/s");
  assert.equal(c.meta, "S2 E4 · 15m left");
  assert.equal(c.progress, 0.7);
  const u = continueCard(upNext, now);
  assert.equal(u.meta, "S1 E3 · Starts from the beginning");
  assert.equal(u.progress, null);
  assert.equal(u.id, "show:12");
});

test("an entry as a strip card", () => {
  const s = stripCard(inProgress, now);
  assert.deepEqual([s.key, s.art, s.line, s.left, s.when, s.progress], ["show:12", "/api/still/e", "A Show · S2 E4", "15m left", "yesterday", 0.7]);
  const u = stripCard(upNext, now);
  assert.equal(u.left, "Starts from the beginning");
  assert.equal(u.progress, null);
  assert.equal(u.line, "A Show · S1 E3");
  assert.equal(stripCard({ ...upNext, episode: { ...upNext.episode, stillUrl: null } }, now).art, "/api/poster/s");
  const m = stripCard(film, now);
  assert.equal(m.art, "/api/poster/m");
  assert.equal(m.title, "A Film");
});

test("counts, first selection and coming back", () => {
  assert.equal(continueCount(1), "1 title");
  assert.equal(continueCount(4), "4 titles");
  assert.equal(firstSelection(false, "*added*", "*added*", 3), CONTINUE);
  assert.equal(firstSelection(true, "*added*", "*added*", 3), null);
  assert.equal(firstSelection(false, "*added*", "*added*", 0), null);
  assert.equal(firstSelection(false, "c4", "*added*", 3), null);
  assert.equal(freshOnReturn(null, now), false);
  assert.equal(freshOnReturn(now - 31_000, now), true);
  assert.equal(freshOnReturn(now - 5_000, now), false);
});
test("the Continue pane shows while it is chosen and no title search is typed", () => {
  assert.equal(continueShown(CONTINUE, ""), true);
  assert.equal(continueShown(CONTINUE, "   "), true, "blank is no search");
  assert.equal(continueShown(CONTINUE, " | "), true, "only pipes is no search");
  assert.equal(continueShown(CONTINUE, "a"), false, "a search is the tab's results");
  assert.equal(continueShown("*added*", ""), false);
  assert.equal(continueShown("c7", ""), false);
});
