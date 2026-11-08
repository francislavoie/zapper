// Up next: which episode follows, and when the card counts down into it.
// Run: node --test src/__checks__/

import assert from "node:assert/strict";
import { test } from "node:test";
import { createAdvanceGuard, nextEpisode, upNextState } from "../logic/upNext.ts";

const ep = (id, season, episode, { end = null } = {}) => ({
  episodeId: id, seriesId: 1, season, episode, episodeEnd: end, title: null, plot: null, durationSecs: 120,
  stillUrl: null, airDate: null, watched: false, position: null, duration: null,
});

const eps = [ep(101, 1, 1), ep(102, 1, 2, { end: 3 }), ep(103, 1, 3), ep(104, 1, 4), ep(201, 2, 1), ep(1, 0, 1)];
test("the next episode is the next in order, past a range, never a special", () => {
  assert.equal(nextEpisode(eps, 101).episodeId, 102);
  assert.equal(nextEpisode(eps, 102).episodeId, 104);   // S1 E2–E3 covers 103
  assert.equal(nextEpisode(eps, 104).episodeId, 201);   // the next season
  assert.equal(nextEpisode(eps, 201), null);            // the last
  assert.equal(nextEpisode(eps, 1), null);              // a special
  assert.equal(nextEpisode([ep(101, 1, 1), { ...ep(102, 1, 2), watched: true }], 101).episodeId, 102);
});
test("an episode the list does not hold has no next", () => {
  assert.equal(nextEpisode(eps, 999), null);
});
const cur = ep(101, 1, 1), nxt = ep(102, 1, 2);
const playing = (position, state = "playing", duration = 120, id = 101) =>
  ({ state, finished: null, onDemand: { kind: "episode", id, position, duration, bufferedTo: null } });
const finished = (duration = 120) => ({ state: "stopped", onDemand: null, finished: { kind: "episode", id: 101, position: 115, duration, bufferedTo: null } });
const up = (p, { cancelled = null, sleeping = false, next = nxt } = {}) => upNextState(p, cur, next, cancelled, sleeping);
test("the card counts the last ten of the last twenty seconds", () => {
  assert.deepEqual(up(playing(95)), { kind: "none" });
  assert.deepEqual(up(playing(100)), { kind: "counting", next: nxt, secondsLeft: 10 });
  assert.equal(up(playing(105.5)).secondsLeft, 5);
  assert.equal(up(playing(105.5, "paused")).secondsLeft, 5);
  assert.deepEqual(up(playing(110)), { kind: "due", next: nxt });
});
test("a finished end is due, whatever its duration", () => {
  assert.equal(up(finished()).kind, "due");
  assert.equal(up(finished(null)).kind, "due");
});
test("never for a drop, a cancel, a sleep timer, another title, no duration or no next", () => {
  assert.equal(up(finished(), { cancelled: 101 }).kind, "none");
  assert.equal(up(playing(60, "failed")).kind, "none");
  assert.equal(up(playing(100), { cancelled: 101 }).kind, "none");
  assert.equal(up(playing(110), { sleeping: true }).kind, "none");
  assert.equal(up(finished(), { sleeping: true }).kind, "none");
  assert.equal(up(playing(110, "playing", 120, 999)).kind, "none");   // the player has another episode open
  assert.equal(up(playing(100, "playing", null)).kind, "none");
  assert.equal(up(playing(100), { next: null }).kind, "none");
});
test("never for a movie, a special, a stop, an opening, or nothing known of what plays", () => {
  const movie = { state: "playing", finished: null, onDemand: { kind: "movie", id: 101, position: 110, duration: 120, bufferedTo: null } };
  assert.equal(up(movie).kind, "none");
  assert.equal(upNextState(playing(110, "playing", 120, 1), ep(1, 0, 1), nxt, null, false).kind, "none");
  assert.equal(up({ state: "stopped", onDemand: null, finished: null }).kind, "none");
  assert.equal(up(playing(110, "opening")).kind, "none");
  assert.equal(upNextState(playing(110), null, nxt, null, false).kind, "none");
  // A finished end of another episode is not this one's.
  assert.equal(up({ ...finished(), finished: { ...finished().finished, id: 999 } }).kind, "none");
  // A cancel for another episode is not this one's.
  assert.equal(up(playing(100), { cancelled: 999 }).kind, "counting");
});
test("the advance is claimed once per episode moved on from", () => {
  const g = createAdvanceGuard();
  assert.equal(g.claim(101), true);
  assert.equal(g.claim(101), false);
  assert.equal(g.claim(102), true);
  // Played again from the sheet: another playing, which may move on again.
  g.reset(101);
  assert.equal(g.claim(101), true);
  assert.equal(g.claim(101), false);
});
