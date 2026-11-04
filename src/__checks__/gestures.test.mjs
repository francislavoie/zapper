// The three zones across the picture, and where their bubbles land.
//
// Worth testing because the boundaries are the whole design: a seek zone that
// crept towards the middle would make a double-click meant for fullscreen
// jump the stream instead, and that is the failure nobody reports precisely —
// they just say it "sometimes does the wrong thing".

import test from "node:test";
import assert from "node:assert/strict";
import {
  SEEK_ZONE_FRACTION,
  zoneAction,
  zoneAt,
  zoneAtFraction,
  zoneCentre,
  zoneIsAvailable,
  zoneSeek,
} from "../logic/gestures.ts";
import { VOD_SKIP_SECONDS } from "../logic/vodTransport.ts";

test("a movie's side zones seek by its own step", () => {
  assert.deepEqual(zoneAction("forward", false, VOD_SKIP_SECONDS), { kind: "seek", zone: "forward", by: 10 });
  assert.deepEqual(zoneAction("back", false, VOD_SKIP_SECONDS), { kind: "seek", zone: "back", by: -10 });
});

test("the sides seek and the middle goes fullscreen", () => {
  assert.equal(zoneAtFraction(0), "back");
  assert.equal(zoneAtFraction(0.14), "back");
  assert.equal(zoneAtFraction(0.5), "middle");
  assert.equal(zoneAtFraction(0.86), "forward");
  assert.equal(zoneAtFraction(1), "forward");
});

test("the middle is the majority of the width", () => {
  // Fullscreen is the gesture people reach for most, and a mis-hit that seeks
  // is worse than one that toggles a thing the same gesture toggles back.
  const middle = 1 - 2 * SEEK_ZONE_FRACTION;
  assert.ok(middle > 2 * SEEK_ZONE_FRACTION / 3, "seek zones have taken over the picture");
  // The boundary belongs to the middle on both sides.
  assert.equal(zoneAtFraction(SEEK_ZONE_FRACTION), "middle");
  assert.equal(zoneAtFraction(1 - SEEK_ZONE_FRACTION), "middle");
});

test("a pointer is placed against the box it was in, not the window", () => {
  // The video is not at the window's origin in browse or stacked layouts.
  assert.equal(zoneAt(920, 900, 400), "back");
  assert.equal(zoneAt(1100, 900, 400), "middle");
  assert.equal(zoneAt(1290, 900, 400), "forward");
});

test("a degenerate box is never read as a seek", () => {
  // Measured before layout, or during a resize. Falling back to the middle
  // means the worst case is a fullscreen toggle, which undoes itself.
  for (const width of [0, -10, Number.NaN]) {
    assert.equal(zoneAt(100, 0, width), "middle");
  }
  assert.equal(zoneAtFraction(Number.NaN), "middle");
});

test("each bubble is centred in its own zone", () => {
  assert.equal(zoneCentre("back"), SEEK_ZONE_FRACTION / 2);
  assert.equal(zoneCentre("forward"), 1 - SEEK_ZONE_FRACTION / 2);
  assert.equal(zoneCentre("middle"), 0.5);
  // Inside the zone it describes, which is what makes it teach the zones.
  assert.equal(zoneAtFraction(zoneCentre("back")), "back");
  assert.equal(zoneAtFraction(zoneCentre("forward")), "forward");
});

test("the zones seek by the step, in the direction they are named", () => {
  assert.equal(zoneSeek("back", 5), -5);
  assert.equal(zoneSeek("forward", 5), 5);
  assert.equal(zoneSeek("middle", 5), null);
});

test("forward is unavailable at the live edge, because there is nowhere to go", () => {
  assert.equal(zoneIsAvailable("forward", true), false);
  assert.equal(zoneIsAvailable("forward", false), true);
  // Back always works: there is always buffer behind, or the seek clamps.
  assert.equal(zoneIsAvailable("back", true), true);
  assert.equal(zoneIsAvailable("middle", true), true);
});
