import test from "node:test";
import assert from "node:assert/strict";
import {
  OPEN_LIMIT_MS,
  PAUSE_RELEASE_MS,
  STALL_LIMIT_MS,
  STUCK_LIMIT,
  NOT_STUCK,
  UNSTUCK_AFTER_S,
  PROGRESS_EVERY_MS,
  bufferedEnd,
  cueTimes,
  movedAt,
  needsData,
  openDue,
  parseVtt,
  positionOf,
  progressDue,
  releaseDue,
  seekPlan,
  stallDue,
  stuckAfterPlay,
  stuckAgain,
  stuckTooOften,
  subtitleAfter,
  progressPath,
  vodStreamUrl,
  watchBody,
} from "../logic/webVod.ts";

test("a remux stream is asked for from a time, with its audio", () => {
  assert.equal(vodStreamUrl("/api/vod/stream/v1", 0, null), "/api/vod/stream/v1");
  assert.equal(vodStreamUrl("/api/vod/stream/v1", 754.4, 2), "/api/vod/stream/v1?from=754&audio=2");
  assert.equal(vodStreamUrl("/api/vod/stream/v1", 0, 0), "/api/vod/stream/v1?audio=0");
});

test("seeking: the browser's own in a direct file, a reopen past what a remux holds", () => {
  assert.deepEqual(seekPlan("direct", 3000, 0, []), { kind: "native", time: 3000 });
  assert.deepEqual(seekPlan("remux", 130, 100, [[0, 60]]), { kind: "native", time: 30 });
  assert.deepEqual(seekPlan("remux", 900, 100, [[0, 60]]), { kind: "reopen", from: 900 });
  assert.deepEqual(seekPlan("remux", 50, 100, [[0, 60]]), { kind: "reopen", from: 50 });
});

test("the place in the film adds where the remux started", () => {
  assert.equal(positionOf("remux", 900, 12.5), 912.5);
  assert.equal(positionOf("direct", 0, 12.5), 12.5);
});

test("progress is reported every ten seconds of play and on pause", () => {
  assert.equal(PROGRESS_EVERY_MS, 10_000);
  assert.equal(progressDue(null, 0, 0.5, false), false);
  assert.equal(progressDue(null, 0, 1.2, false), true);
  assert.equal(progressDue({ at: 0, position: 1.2 }, 9_000, 10, false), false);
  assert.equal(progressDue({ at: 0, position: 1.2 }, 10_000, 11, false), true);
  assert.equal(progressDue({ at: 0, position: 1.2 }, 2_000, 3, true), true);
  // Once a pause has been told, the same pause is not told again.
  assert.equal(progressDue({ at: 2_000, position: 3 }, 30_000, 3, true), false);
});

test("a paused film lets the account's connection go after a minute", () => {
  assert.equal(PAUSE_RELEASE_MS, 60_000);
  assert.equal(releaseDue(null, 1_000_000), false, "playing");
  assert.equal(releaseDue(1_000, 60_999), false);
  assert.equal(releaseDue(1_000, 61_000), true);
});

test("where the picture's data ends, in the film's time", () => {
  assert.equal(bufferedEnd([], 5, 100), null);
  assert.equal(bufferedEnd([[0, 30], [40, 80]], 45, 100), 180);
  assert.equal(bufferedEnd([[0, 30], [40, 80]], 10, 0), 30);
  // Between ranges: the next one ahead is where playing goes on.
  assert.equal(bufferedEnd([[0, 30], [40, 80]], 35, 0), 80);
});

test("subtitles: asked for from a while before the last cue held, else from where the picture is", () => {
  assert.equal(subtitleAfter(null, 754.6), 754.6);
  // A cue decoded later that ends before the last one held is still
  // fetched; those already held are not added twice.
  assert.equal(subtitleAfter(812.25, 790), 802.25);
});

test("a film that doesn't start within half a minute is given up on", () => {
  assert.equal(OPEN_LIMIT_MS, 30_000);
  assert.equal(openDue(1_000, 30_999), false);
  assert.equal(openDue(1_000, 31_000), true);
});

test("a playing film whose place stands still for half a minute has stalled", () => {
  assert.equal(STALL_LIMIT_MS, 30_000);
  const first = movedAt(null, 1_000, 10, false);
  assert.deepEqual(first, { at: 1_000, position: 10 });
  // Standing still keeps the time it last moved.
  assert.equal(movedAt(first, 20_000, 10, false), first);
  assert.equal(stallDue(movedAt(first, 31_000, 10, false), 31_000), true);
  assert.equal(stallDue(first, 30_999), false);
  // Moving, or paused, starts the count again.
  assert.deepEqual(movedAt(first, 20_000, 10.5, false), { at: 20_000, position: 10.5 });
  assert.deepEqual(movedAt(first, 40_000, 10, true), { at: 40_000, position: 10 });
});

test("a film stuck three times in a row is given up on; reopened twice, it is not", () => {
  assert.equal(STUCK_LIMIT, 2);
  let st = NOT_STUCK;
  st = stuckAgain(st);
  assert.equal(stuckTooOften(st), false);
  st = stuckAgain(st);
  assert.equal(stuckTooOften(st), false);
  st = stuckAgain(st);
  assert.equal(stuckTooOften(st), true);
});

test("being stuck ends only after ten seconds of play past where it stuck", () => {
  assert.equal(UNSTUCK_AFTER_S, 10);
  let st = stuckAgain(NOT_STUCK);
  // A twitch after a reopen is not playing again.
  st = stuckAfterPlay(st, 100, 100.25);
  assert.equal(st.count, 1);
  for (let t = 100.25; t < 109.5; t += 0.25) st = stuckAfterPlay(st, t, t + 0.25);
  assert.equal(st.count, 1, `played ${st.played}`);
  st = stuckAfterPlay(st, 109.5, 110);
  assert.deepEqual(st, NOT_STUCK);
  // A seek, forward or back, is not play.
  st = stuckAgain(NOT_STUCK);
  st = stuckAfterPlay(st, 100, 400);
  st = stuckAfterPlay(st, 400, 50);
  assert.deepEqual(st, { count: 1, played: 0 });
  // Sticking again starts the count of play over.
  st = stuckAfterPlay(st, 50, 51);
  st = stuckAgain(st);
  assert.deepEqual(st, { count: 2, played: 0 });
});

test("a film the service let go is watched again only when the element wants more", () => {
  assert.equal(needsData(4), false, "playing from what it holds");
  assert.equal(needsData(3), false);
  assert.equal(needsData(2), true, "waiting for data");
  assert.equal(needsData(1), true);
});

test("a cue in the film's time is put on the element's clock, which starts at the remux's start", () => {
  assert.deepEqual(cueTimes({ start: 912.5, end: 915, text: "x" }, 900), { start: 12.5, end: 15 });
  assert.deepEqual(cueTimes({ start: 12.5, end: 15, text: "x" }, 0), { start: 12.5, end: 15 });
  // A cue begun before the stream did shows from its first moment.
  assert.deepEqual(cueTimes({ start: 898, end: 903, text: "x" }, 900), { start: 0, end: 3 });
  assert.equal(cueTimes({ start: 890, end: 899, text: "x" }, 900), null);
});

test("the service's WebVTT is read into cues", () => {
  const doc =
    "WEBVTT\n\n00:00:10.000 --> 00:00:12.500\nFirst line\nsecond line\n\n" +
    "01:02:03.250 --> 01:02:05.000\nLater\n\n";
  assert.deepEqual(parseVtt(doc), [
    { start: 10, end: 12.5, text: "First line\nsecond line" },
    { start: 3723.25, end: 3725, text: "Later" },
  ]);
  assert.deepEqual(parseVtt("WEBVTT\n\n"), []);
  assert.deepEqual(parseVtt("WEBVTT\r\n\r\n00:01.000 --> 00:02.000\r\nShort form\r\n"), [
    { start: 1, end: 2, text: "Short form" },
  ]);
  assert.deepEqual(parseVtt("not a subtitle file"), []);
});

test("a watch names the title it asks for: an episode or a movie, not both", () => {
  const rest = { anyway: false, replaces: null, audio: null, can: {} };
  const ep = watchBody({ kind: "episode", id: 7 }, rest);
  assert.equal(ep.episodeId, 7);
  assert.equal("movieId" in ep, false);
  assert.equal(ep.anyway, false);
  assert.equal(ep.replaces, null);
  assert.equal(ep.audio, null);
  assert.deepEqual(ep.can, {});
  const mv = watchBody({ kind: "movie", id: 7 }, { ...rest, anyway: true, replaces: "v1", audio: 2 });
  assert.equal(mv.movieId, 7);
  assert.equal("episodeId" in mv, false);
  assert.equal(mv.anyway, true);
  assert.equal(mv.replaces, "v1");
  assert.equal(mv.audio, 2);
});

test("a title's place is reported to its own kind's address", () => {
  assert.equal(progressPath({ kind: "episode", id: 7 }), "/api/vod/episodes/7/progress");
  assert.equal(progressPath({ kind: "movie", id: 7 }), "/api/vod/movies/7/progress");
});
