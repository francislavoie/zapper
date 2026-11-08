// Where a tooltip lands. The window can be very small, and the targets that
// need tips most sit against its edges — the title bar's buttons along the
// top, the player's bar along the bottom, the close button in the corner.
// Every case here is one where a naive "centred below" would put the tip
// partly or wholly off screen.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { TIP_GAP, TIP_MARGIN, WIDE_TARGET, placeTip, tipAnchor } from "../logic/tooltip.ts";

const box = (left, top, width, height) => ({
  left,
  top,
  right: left + width,
  bottom: top + height,
});

/** Inside the window, margins included. */
function onScreen(p, w, h, vw, vh) {
  return (
    p.left >= TIP_MARGIN &&
    p.top >= TIP_MARGIN &&
    p.left + w <= vw - TIP_MARGIN &&
    p.top + h <= vh - TIP_MARGIN
  );
}

test("below and centred when there is room", () => {
  const p = placeTip(box(400, 100, 40, 20), 100, 24, 1200, 800);
  assert.equal(p.side, "below");
  assert.equal(p.top, 120 + TIP_GAP);
  assert.equal(p.left, 420 - 50);
});

test("a control along the bottom flips the tip above it", () => {
  const p = placeTip(box(400, 770, 40, 24), 100, 24, 1200, 800);
  assert.equal(p.side, "above");
  assert.equal(p.top, 770 - TIP_GAP - 24);
  assert.ok(onScreen(p, 100, 24, 1200, 800));
});

test("the close button in the corner keeps its tip inside the window", () => {
  const p = placeTip(box(1170, 6, 28, 28), 80, 24, 1200, 800);
  assert.equal(p.side, "below");
  assert.equal(p.left, 1200 - TIP_MARGIN - 80);
  assert.ok(onScreen(p, 80, 24, 1200, 800));
});

test("a target at the left edge slides the tip right, not off", () => {
  const p = placeTip(box(0, 300, 20, 20), 200, 24, 1200, 800);
  assert.equal(p.left, TIP_MARGIN);
});

test("a tall tip in a short window takes the roomier side and is clamped on", () => {
  // 200px window, a description tip 150px tall, target in the upper middle.
  const p = placeTip(box(50, 60, 100, 20), 180, 150, 400, 200);
  assert.equal(p.side, "below");
  assert.ok(onScreen(p, 180, 150, 400, 200), JSON.stringify(p));
});

test("a tip wider than the window starts at the margin", () => {
  // The first words are the ones that matter; it runs off the far side.
  const p = placeTip(box(100, 50, 40, 20), 500, 24, 300, 400);
  assert.equal(p.left, TIP_MARGIN);
});

// Wide targets: a row, a heading. Centred on one, the tip lands in the middle
// of the window, nowhere near the text or the pointer.

test("a cut-short name's tip starts where the name does, however wide its box", () => {
  const row = box(220, 480, 940, 18);
  const anchor = tipAnchor(row, true, 228, 700);
  assert.deepEqual(anchor, { x: 228, align: "start" });
  const p = placeTip(row, 300, 24, 1280, 800, anchor);
  assert.equal(p.left, 228);
});

test("any other tip over a wide target goes by the pointer", () => {
  const row = box(0, 300, 1000, 40);
  const anchor = tipAnchor(row, false, 10, 640);
  assert.deepEqual(anchor, { x: 640, align: "centre" });
  assert.equal(placeTip(row, 200, 24, 1280, 800, anchor).left, 540);
});

test("from the keyboard, over a wide target, it goes by the text", () => {
  assert.deepEqual(tipAnchor(box(0, 300, 1000, 40), false, 12, null), { x: 12, align: "start" });
});

test("a control's tip still centres on it", () => {
  const button = box(400, 100, 28, 28);
  assert.equal(tipAnchor(button, false, 404, 410), null);
  assert.equal(tipAnchor(box(0, 0, WIDE_TARGET, 20), false, 0, 5), null);
});

test("an anchored tip stays inside the window", () => {
  const near = placeTip(box(0, 100, 1280, 20), 300, 24, 1280, 800, { x: 1270, align: "centre" });
  assert.equal(near.left, 1280 - TIP_MARGIN - 300);
  const start = placeTip(box(1100, 100, 170, 20), 300, 24, 1280, 800, { x: 1100, align: "start" });
  assert.equal(start.left, 1280 - TIP_MARGIN - 300);
});

test("a tip shows only when the text is cut short by more than rounding", () => {
  // WebKit can round a fractional box's scroll width up and its client
  // width down: a pixel apart with nothing hidden. The difference alone
  // must not decide it (src/tooltip.ts isTruncated).
  const src = readFileSync(new URL("../tooltip.ts", import.meta.url), "utf8");
  assert.match(src, /el\.scrollWidth - el\.clientWidth > 1/);
  assert.doesNotMatch(src, /el\.scrollWidth > el\.clientWidth/);
});
