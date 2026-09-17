// "Is there room to stack?" — the predicate behind the chevron in the middle
// of the player bar. Pure arithmetic on two window dimensions, and the only
// thing standing between the layout and a letterbox over three rows of guide.
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  STACK_ASPECT,
  STACK_SPARE,
  canStack,
  parseStacked,
  stackedVideoHeight,
} from "../logic/stack.ts";

/** The exact stage height at which a window of this width tips over. */
const threshold = (width) => (1 + STACK_SPARE) * stackedVideoHeight(width);

/** What the app passes: the window, minus its own title bar. The bar is 38 CSS
 *  pixels (TOOLBAR_HEIGHT, mirrored by --toolbar-h and asserted against the
 *  live bar elsewhere); it is spelled out here so this file imports nothing
 *  but the module under test. */
const BAR = 38;
const stage = (height) => height - BAR;

test("a full-width picture is as tall as its own shape says", () => {
  assert.equal(stackedVideoHeight(1600), 900);
  assert.equal(stackedVideoHeight(1600), 1600 / STACK_ASPECT);
  // Nothing sensible can be done with a zero-width window, and it must not
  // divide by anything.
  assert.equal(stackedVideoHeight(0), 0);
  assert.equal(stackedVideoHeight(-100), 0);
});

test("a landscape window never stacks, however tall it is", () => {
  // Taller than wide, first of all.
  assert.equal(canStack(1920, 1080, stage(1080)), false);
  assert.equal(canStack(3840, 2160, stage(2160)), false);
  // A square window is not taller than wide.
  assert.equal(canStack(1000, 1000, stage(1000)), false);
});

test("a portrait window with room to spare stacks", () => {
  // 900 wide: the picture is 506 tall and the guide gets about 1500 — three
  // times what the rule asks for.
  const h = 2000 + BAR;
  assert.equal(canStack(900, h, stage(h)), true);
});

test("a portrait window that is only just portrait does not", () => {
  // Taller than wide, but the picture plus half again does not fit: this is
  // the case the predicate exists for, where the control would appear and
  // produce a letterbox over a sliver of guide.
  assert.equal(canStack(1000, 1100, stage(1100)), false);
});

test("the crossover is exactly one and a half picture-heights of spare room", () => {
  const width = 800;
  const need = threshold(width); // stage height, not window height
  assert.equal(canStack(width, width + 1, need), true, "at the threshold it fits");
  assert.equal(canStack(width, width + 1, need - 1), false, "a pixel under does not");
  assert.equal(canStack(width, width + 1, need + 1), true, "a pixel over does");
});

test("the title bar comes out of the room available, not out of the window", () => {
  const width = 800;
  const need = threshold(width);
  // A window exactly `need` tall has less than that below the bar, so it must
  // not stack; one that is `need + BAR` tall must. This is the difference the
  // caller's subtraction makes, and it is the difference between the picture
  // fitting and the picture being drawn behind opaque chrome.
  assert.equal(canStack(width, need, stage(need)), false);
  assert.equal(canStack(width, need + BAR, stage(need + BAR)), true);
});

test("the measured stage decides, not the window height", () => {
  const width = 800;
  // The window is enormous, but the caller measured a stage with no room in
  // it. The measurement wins — it is the space actually being divided up.
  assert.equal(canStack(width, 5000, 100), false);
  assert.equal(canStack(width, 5000, threshold(width)), true);
});

test("degenerate sizes answer no rather than throwing or dividing by zero", () => {
  for (const [w, h, s] of [
    [0, 0],
    [0, 1000],
    [1000, 0],
    [-100, -100],
    [Number.NaN, 1000],
    [1000, Number.NaN],
    // A window with room, but a stage nobody could measure.
    [1000, 2000, Number.NaN],
    [1000, 2000, 0],
  ]) {
    assert.equal(canStack(w, h, s ?? stage(h)), false, `${w}x${h} stage ${s}`);
  }
});

test("the answer does not change with page zoom", () => {
  // Zoom scales the CSS pixel, so every length shrinks together and the
  // ratios this predicate tests are unchanged.
  const at = (z) => canStack(900 / z, 2000 / z, 1960 / z);
  assert.equal(at(1), true);
  assert.equal(at(1.25), true);
  assert.equal(at(2), true);
});

test("only the string this app writes counts as remembered", () => {
  assert.equal(parseStacked("1"), true);
  assert.equal(parseStacked("0"), false);
  assert.equal(parseStacked(null), false);
  // A corrupt or foreign value must not decide the layout.
  for (const junk of ["true", "yes", "", "null", "{}"]) {
    assert.equal(parseStacked(junk), false, junk);
  }
});
