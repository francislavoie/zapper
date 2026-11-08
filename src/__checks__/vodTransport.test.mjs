import test from "node:test";
import assert from "node:assert/strict";
import {
  VOD_SKIP_SECONDS,
  bufferedFraction,
  isOnDemandKind,
  playedFraction,
  seekTarget,
  vodLabel,
  vodTimes,
} from "../logic/vodTransport.ts";

const od = (position, duration, bufferedTo = null) => ({ kind: "movie", id: 1, position, duration, bufferedTo });

test("times read elapsed, total and what is left", () => {
  assert.deepEqual(vodTimes(3492, 7264), { elapsed: "58:12", total: "2:01:04", remaining: "−1:02:52" });
  assert.deepEqual(vodTimes(65, null), { elapsed: "1:05", total: "", remaining: "" });
  assert.equal(vodLabel(od(3492, 7264)), "58:12 / 2:01:04 · −1:02:52");
  assert.equal(vodLabel(od(3492, 7264), true), "58:12 / 2:01:04");
  assert.equal(vodLabel(od(65, null)), "1:05");
});

test("a seek lands inside the movie", () => {
  assert.equal(seekTarget(0.5, 7200), 3600);
  assert.equal(seekTarget(-0.2, 7200), 0);
  assert.equal(seekTarget(1.4, 7200), 7200);
  assert.equal(seekTarget(0.5, null), null);
});

test("fills: played and buffered as fractions", () => {
  assert.equal(playedFraction(od(30, 120)), 0.25);
  assert.equal(bufferedFraction(od(30, 120, 90)), 0.75);
  assert.equal(playedFraction(od(30, null)), 0);
  assert.equal(bufferedFraction(od(30, 120)), 0);
});

test("a movie skips ten seconds", () => {
  assert.equal(VOD_SKIP_SECONDS, 10);
});

test("a movie and an episode get the on-demand transport; a channel and nothing do not", () => {
  assert.equal(isOnDemandKind("movie"), true);
  assert.equal(isOnDemandKind("episode"), true);
  assert.equal(isOnDemandKind("channel"), false);
  assert.equal(isOnDemandKind(null), false);
});
