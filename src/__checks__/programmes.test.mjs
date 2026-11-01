import test from "node:test";
import assert from "node:assert/strict";
import {
  airingTag,
  airingWhen,
  centredScrollTop,
  focusCategory,
  guideOffsetFor,
  hitAction,
  hitTip,
  inGuideReach,
} from "../logic/programmes.ts";
import { GUIDE_FUTURE_HOURS, GUIDE_PAST_HOURS, HALF_HOUR, HOUR, anchorFor } from "../logic/guide.ts";

/** Local noon on a fixed day, so day names do not depend on when this runs. */
const NOON = Math.floor(new Date(2026, 8, 8, 12, 0, 0).getTime() / 1000);
const at = (h, m = 0) => NOON + h * HOUR + m * 60;

test("each airing does its own thing", () => {
  assert.equal(hitAction("live"), "tune");
  assert.equal(hitAction("upcoming"), "guide");
  assert.equal(hitAction("catchup"), "catchup");
  for (const a of ["live", "upcoming", "catchup"]) {
    assert.ok(airingTag(a).length <= 8, "the palette's first column is narrow");
    assert.ok(hitTip(a).length > 0);
  }
});

test("on now says until when", () => {
  const hit = { airing: "live", programme: { start: at(-1), stop: at(0, 30) } };
  assert.equal(airingWhen(hit, NOON), "until 12:30");
  // A guide entry with no stop has no end to give, rather than "until 00:00".
  const open = { airing: "live", programme: { start: at(-1), stop: 0 } };
  assert.equal(airingWhen(open, NOON), "now");
});

test("upcoming and catch-up name the day", () => {
  const when = (airing, start) => airingWhen({ airing, programme: { start, stop: start + HOUR } }, NOON);
  assert.equal(when("upcoming", at(8)), "Today 20:00");
  assert.equal(when("upcoming", at(18)), "Tomorrow 06:00");
  assert.equal(when("catchup", at(-15)), "Yesterday 21:00");
  // Further out, a weekday — 2026-09-11 is a Friday.
  const fri = when("upcoming", at(72));
  assert.match(fri, / 12:00$/);
  assert.notEqual(fri.split(" ")[0], "Today");
  assert.notEqual(fri.split(" ")[0], "Tomorrow");
});

test("the guide opens on the half hour the programme starts in", () => {
  const anchor = anchorFor(NOON);
  assert.equal(guideOffsetFor(at(8), anchor, 3), 8 * HOUR);
  assert.equal(guideOffsetFor(at(8, 45), anchor, 3), 8 * HOUR + HALF_HOUR);
  // Starting in a few minutes: the window the clock is in.
  assert.equal(guideOffsetFor(at(0, 10), anchor, 3), 0);
});

test("the offset stays inside the guide's reach", () => {
  const anchor = anchorFor(NOON);
  const far = guideOffsetFor(at(24 * 6), anchor, 3);
  assert.equal(far, (GUIDE_FUTURE_HOURS - 3) * HOUR);
  const old = guideOffsetFor(at(-24 * 5), anchor, 3);
  assert.equal(old, -GUIDE_PAST_HOURS * HOUR);
});

test("reach says when the guide cannot show an airing", () => {
  assert.ok(inGuideReach(at(20), NOON));
  assert.ok(inGuideReach(at(GUIDE_FUTURE_HOURS - 1), NOON));
  assert.ok(!inGuideReach(at(GUIDE_FUTURE_HOURS + 1), NOON));
});

test("the guide keeps the category on screen when the channel is in it", () => {
  const ALL = "*all*";
  assert.equal(focusCategory(ALL, "sports", ALL), ALL);
  assert.equal(focusCategory("sports", "sports", ALL), "sports");
  assert.equal(focusCategory("news", "sports", ALL), "sports");
  // A channel in no category is only in "all".
  assert.equal(focusCategory("news", null, ALL), ALL);
});

test("a focused row is centred, never above the top", () => {
  assert.equal(centredScrollTop(100, 34, 340), 100 * 34 - 153);
  assert.equal(centredScrollTop(0, 34, 340), 0);
  assert.equal(centredScrollTop(2, 34, 340), 0);
});
