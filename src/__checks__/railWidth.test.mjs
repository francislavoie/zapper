// The divider between the rail and the channel list.
//
// Same shape as the picture's width, and the same reason for testing it: the
// value is dragged, and a drag produces a continuous stream of nonsense at
// the edges. The one thing that differs is the direction.

import test from "node:test";
import assert from "node:assert/strict";
import {
  RAIL_WIDTH_DEFAULT,
  RAIL_WIDTH_MAX,
  RAIL_WIDTH_MIN,
  clampRailWidth,
  isDefaultRailWidth,
  parseRailWidth,
  railAfterDrag,
} from "../logic/railWidth.ts";

test("the default sits inside the bounds", () => {
  assert.ok(RAIL_WIDTH_MIN < RAIL_WIDTH_DEFAULT);
  assert.ok(RAIL_WIDTH_DEFAULT < RAIL_WIDTH_MAX);
  assert.equal(clampRailWidth(RAIL_WIDTH_DEFAULT), RAIL_WIDTH_DEFAULT);
  assert.equal(isDefaultRailWidth(parseRailWidth(String(RAIL_WIDTH_DEFAULT))), true);
});

test("the rail grows rightwards, unlike the picture", () => {
  // It is pinned to the left edge, so dragging its right edge away from the
  // origin makes it wider. The picture's handle is the other way round.
  assert.equal(railAfterDrag(16, 32, 16), 18);
  assert.equal(railAfterDrag(16, -32, 16), 14);
  assert.equal(railAfterDrag(16, 0, 16), 16);
});

test("a drag measured under CSS zoom still tracks the pointer", () => {
  // At 150% the rail measures 24 client pixels per rem, not 16.
  assert.equal(railAfterDrag(16, 48, 24), 18);
});

test("a drag cannot escape the bounds or divide by nothing", () => {
  assert.equal(railAfterDrag(16, 100_000, 16), RAIL_WIDTH_MAX);
  assert.equal(railAfterDrag(16, -100_000, 16), RAIL_WIDTH_MIN);
  // Measured before layout: no scale to drag against, so leave it alone.
  assert.equal(railAfterDrag(16, 80, 0), 16);
  assert.equal(railAfterDrag(16, Number.NaN, 16), 16);
});

test("widths are quantised and rubbish takes the default", () => {
  assert.equal(clampRailWidth(16.1), 16);
  assert.equal(clampRailWidth(16.13), 16.25);
  assert.equal(clampRailWidth(Number.NaN), RAIL_WIDTH_DEFAULT);
  assert.equal(parseRailWidth(null), RAIL_WIDTH_DEFAULT);
  assert.equal(parseRailWidth("not a number"), RAIL_WIDTH_DEFAULT);
  assert.equal(parseRailWidth("999"), RAIL_WIDTH_MAX);
});

test("the rail cannot be narrowed until the names are gone", () => {
  // Below about nine rem every category name is an ellipsis, which makes
  // the rail useless rather than compact.
  assert.ok(RAIL_WIDTH_MIN >= 8);
  // Nor widened until the list it navigates has no room to be a list.
  assert.ok(RAIL_WIDTH_MAX <= 40);
});
