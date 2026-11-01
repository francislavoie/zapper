// How big the docked picture is.
//
// The arithmetic matters because the value is dragged, and a drag produces a
// continuous stream of nonsense at the edges: past the bounds, past zero,
// and — if the pixels-per-rem measurement is taken before layout — divided
// by nothing at all.

import test from "node:test";
import assert from "node:assert/strict";
import {
  VIDEO_GRIP_W,
  VIDEO_INFO_MIN,
  VIDEO_LIST_MIN,
  VIDEO_WIDTH_DEFAULT,
  VIDEO_WIDTH_MAX,
  VIDEO_WIDTH_MIN,
  clampVideoWidth,
  fittedVideoWidth,
  videoRoomRem,
  isDefaultVideoWidth,
  parseVideoWidth,
  widthAfterDrag,
} from "../logic/videoSize.ts";
import { readFileSync } from "node:fs";

const css = readFileSync(new URL("../styles.css", import.meta.url), "utf8");

/** A declaration's value in the first top-level rule for `selector` that
 *  sets it. */
function declared(selector, property) {
  const head = `\n${selector} {`;
  for (let at = css.indexOf(head); at >= 0; at = css.indexOf(head, at + 1)) {
    const body = css.slice(at, css.indexOf("}", at));
    const m = body.match(new RegExp(`\\n\\s*${property}:\\s*([^;]+);`));
    if (m) return m[1].trim();
  }
  assert.fail(`${selector} sets no ${property}`);
}

test("the stored width has no layout ceiling of its own", () => {
  // The window decides how big the picture is drawn. A fixed maximum held a
  // maximised picture well short of what the screen had room for.
  assert.ok(VIDEO_WIDTH_MAX >= 200, `a ceiling of ${VIDEO_WIDTH_MAX}rem would cap big screens`);
});

test("the panel beside the picture keeps room for its widest column", () => {
  // The facts column's basis is the width at which the measured line is
  // never cut short; the panel adds a rem of padding either side.
  const basis = Number.parseFloat(declared(".watch-col.facts", "flex").split(/\s+/)[2]);
  const pad = Number.parseFloat(declared(".watch-info", "padding").split(/\s+/)[1]);
  assert.ok(VIDEO_INFO_MIN >= basis + 2 * pad, `${VIDEO_INFO_MIN} < ${basis} + 2 × ${pad}`);
  // And not so much that the picture stops being able to get big.
  assert.ok(VIDEO_INFO_MIN <= 24, `${VIDEO_INFO_MIN}rem beside the picture is more than it needs`);
});

test("the handle's width is the one the stylesheet draws", () => {
  assert.equal(declared(".watch-grip", "width"), `${VIDEO_GRIP_W}rem`);
});

test("a wide window lets the picture grow until the panel is at its minimum", () => {
  // 2560×1600 at 100%: 160 × 97.6rem of stage. Tall enough that width is
  // what decides, and far past the old fixed 72rem.
  const room = videoRoomRem(160, 97.6);
  // Down to the quarter rem the width moves in.
  assert.equal(room, Math.floor((160 - VIDEO_INFO_MIN - VIDEO_GRIP_W) * 4) / 4);
  assert.ok(room > 72);
  assert.equal(fittedVideoWidth(200, room), room);
});

test("a short window lets the height decide, keeping the picture 16:9", () => {
  // 1920×1080 maximised: 120 × 65.1rem of stage. The list's strip runs out
  // before the panel's column does.
  const room = videoRoomRem(120, 65.125);
  assert.ok(Math.abs(room - ((65.125 - VIDEO_LIST_MIN) * 16) / 9) < 0.25, `room ${room}`);
  assert.ok(120 - room - VIDEO_GRIP_W > VIDEO_INFO_MIN, "the width was the limit after all");
  assert.ok(65.125 - (room * 9) / 16 >= VIDEO_LIST_MIN);
});

test("a big chosen size shrinks with the window and comes back when it widens", () => {
  const chosen = 150;
  const sizes = [
    [160, 87.6],
    [100, 60],
    [70, 45],
    [160, 87.6],
  ];
  const shown = sizes.map(([w, h]) => fittedVideoWidth(chosen, videoRoomRem(w, h)));
  for (const [i, [w, h]] of sizes.entries()) {
    assert.ok(w - shown[i] - VIDEO_GRIP_W >= VIDEO_INFO_MIN - 1e-9, `panel crushed at ${w}×${h}`);
    assert.ok(h - (shown[i] * 9) / 16 >= VIDEO_LIST_MIN - 1e-9, `list crushed at ${w}×${h}`);
  }
  assert.ok(shown[1] < shown[0] && shown[2] < shown[1], `${shown}`);
  assert.equal(shown[3], shown[0], "the size did not come back");
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

test("a smaller window draws less of the picture without forgetting its size", () => {
  // Chosen big in a big window: 60rem.
  const chosen = 60;
  // Reopened in a ~900x560 window: about 56rem x 33rem of stage.
  const room = videoRoomRem(56, 33);
  const shown = fittedVideoWidth(chosen, room);
  assert.ok(shown < chosen, "the oversized picture was drawn anyway");
  // The list keeps a usable strip under the band...
  assert.ok(33 - (shown * 9) / 16 >= 12 - 1e-9, `band too tall: ${shown}`);
  // ...and the information beside it keeps its column.
  assert.ok(56 - shown >= 14 - 1e-9, `info column crushed: ${shown}`);
  // The chosen width is untouched: back in a big window it all comes back.
  assert.equal(fittedVideoWidth(chosen, videoRoomRem(150, 90)), chosen);
});

test("an unmeasured stage puts no ceiling on the picture", () => {
  assert.equal(videoRoomRem(0, 0), Infinity);
  assert.equal(videoRoomRem(Number.NaN, 40), Infinity);
  assert.equal(fittedVideoWidth(30, Infinity), 30);
});

test("a window too small for even the minimum still draws the minimum", () => {
  assert.equal(fittedVideoWidth(30, videoRoomRem(20, 10)), VIDEO_WIDTH_MIN);
});

test("room is quantised down, so what is drawn always fits", () => {
  const room = videoRoomRem(100, 33.3);
  assert.equal(room * 4, Math.floor(room * 4));
  assert.ok(33.3 - (room * 9) / 16 >= 12);
});

test("a big window allows a big picture", () => {
  // 1920x1080 maximised at 16px/rem: about 120 x 65rem of stage. A
  // half-height cap held the picture to ~58rem here, and a fixed maximum to
  // 72rem; the band may take all but the list's strip.
  assert.ok(videoRoomRem(120, 65) > 90, `room ${videoRoomRem(120, 65)}`);
  assert.equal(fittedVideoWidth(90, videoRoomRem(120, 65)), 90);
});
