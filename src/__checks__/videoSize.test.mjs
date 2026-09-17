// How big the docked picture is.
//
// The arithmetic matters because the value is dragged, and a drag produces a
// continuous stream of nonsense at the edges: past the bounds, past zero,
// and — if the pixels-per-rem measurement is taken before layout — divided
// by nothing at all.

import test from "node:test";
import assert from "node:assert/strict";
import {
  VIDEO_WIDTH_DEFAULT,
  VIDEO_WIDTH_MAX,
  VIDEO_WIDTH_MIN,
  clampVideoWidth,
  isDefaultVideoWidth,
  parseVideoWidth,
  widthAfterDrag,
} from "../logic/videoSize.ts";

test("the picture can take most of the band", () => {
  // The information beside it wraps rather than being crushed, so the
  // ceiling is about leaving something rather than about leaving room for
  // a particular layout.
  assert.ok(VIDEO_WIDTH_MAX >= 60, "the picture cannot be made large enough to be worth it");
});

test("the default sits inside the bounds", () => {
  assert.ok(VIDEO_WIDTH_MIN < VIDEO_WIDTH_DEFAULT);
  assert.ok(VIDEO_WIDTH_DEFAULT < VIDEO_WIDTH_MAX);
  assert.equal(clampVideoWidth(VIDEO_WIDTH_DEFAULT), VIDEO_WIDTH_DEFAULT);
});

test("widths are clamped and quantised", () => {
  assert.equal(clampVideoWidth(1), VIDEO_WIDTH_MIN);
  assert.equal(clampVideoWidth(1000), VIDEO_WIDTH_MAX);
  // Quarter-rem steps, so a stored value round-trips through a string
  // exactly and two drags to the same place agree.
  assert.equal(clampVideoWidth(20.1), 20);
  assert.equal(clampVideoWidth(20.13), 20.25);
  assert.equal(clampVideoWidth(20.99), 21);
  // Nonsense takes the default rather than the floor: a broken number says
  // nothing about which end the user wanted.
  assert.equal(clampVideoWidth(Number.NaN), VIDEO_WIDTH_DEFAULT);
  assert.equal(clampVideoWidth(Number.POSITIVE_INFINITY), VIDEO_WIDTH_DEFAULT);
});

test("a stored width is read back, and a broken one is not", () => {
  assert.equal(parseVideoWidth(null), VIDEO_WIDTH_DEFAULT);
  assert.equal(parseVideoWidth(""), VIDEO_WIDTH_DEFAULT);
  assert.equal(parseVideoWidth("26.5"), 26.5);
  assert.equal(parseVideoWidth("not a number"), VIDEO_WIDTH_DEFAULT);
  assert.equal(parseVideoWidth("999"), VIDEO_WIDTH_MAX);
});

test("dragging the left edge outwards makes the picture bigger", () => {
  // 16 pixels to a rem, dragged 32px left: two rem wider.
  assert.equal(widthAfterDrag(20, -32, 16), 22);
  // And rightwards, smaller.
  assert.equal(widthAfterDrag(20, 32, 16), 18);
  assert.equal(widthAfterDrag(20, 0, 16), 20);
});

test("a drag measured under CSS zoom still tracks the pointer", () => {
  // At 150% the slot measures 24 client pixels per rem, not 16. Using 16
  // would move the edge half again as far as the pointer.
  assert.equal(widthAfterDrag(20, -48, 24), 22);
});

test("a drag cannot escape the bounds or divide by nothing", () => {
  assert.equal(widthAfterDrag(20, -100_000, 16), VIDEO_WIDTH_MAX);
  assert.equal(widthAfterDrag(20, 100_000, 16), VIDEO_WIDTH_MIN);
  // Measured before layout: the slot has no width, so there is no scale to
  // drag against and the width is left alone.
  assert.equal(widthAfterDrag(20, -80, 0), 20);
  assert.equal(widthAfterDrag(20, Number.NaN, 16), 20);
});

test("the reset affordance knows when there is nothing to reset", () => {
  assert.equal(isDefaultVideoWidth(VIDEO_WIDTH_DEFAULT), true);
  // Round-tripped through a string, which is how it comes back from storage.
  assert.equal(isDefaultVideoWidth(parseVideoWidth(String(VIDEO_WIDTH_DEFAULT))), true);
  assert.equal(isDefaultVideoWidth(VIDEO_WIDTH_DEFAULT + 2), false);
});
