// Spans of time, compact: "42s", "4m", "11m12s", "1h05m". Unit letters, so a
// length never reads as a clock time the way "11:12" did.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import { duration } from "../logic/format.ts";

test("under a minute is seconds", () => {
  assert.equal(duration(0), "0s");
  assert.equal(duration(42.4), "42s");
  assert.equal(duration(59.4), "59s");
});

test("minutes and seconds under an hour, padded, exact minutes bare", () => {
  assert.equal(duration(59.6), "1m");
  assert.equal(duration(240), "4m");
  assert.equal(duration(672), "11m12s");
  assert.equal(duration(65), "1m05s");
});

test("hours and minutes past an hour", () => {
  assert.equal(duration(3600), "1h");
  assert.equal(duration(3900), "1h05m");
  assert.equal(duration(3599.6), "1h");
  assert.equal(duration(7 * 3600 + 29 * 60 + 40), "7h30m");
});

test("never looks like a clock", () => {
  for (const s of [5, 65, 672, 3900, 86_000]) {
    assert.doesNotMatch(duration(s), /:/, `${s}s came out as ${duration(s)}`);
  }
});

test("nonsense clamps to zero", () => {
  assert.equal(duration(-3), "0s");
  assert.equal(duration(Number.NaN), "0s");
});
