// Video placement is arithmetic, and arithmetic can be checked without eyes.
// Run: node --test src/__checks__/
//
// These import the .ts modules directly; Node 26 strips the types.

import test from "node:test";
import assert from "node:assert/strict";
import {
  FULL,
  easeOutCubic,
  fitAspect,
  lerpRect,
  normalisedRect,
  rectAt,
  rectEquals,
  shouldSend,
} from "../logic/rect.ts";

const near = (a, b, eps = 1e-9) =>
  assert.ok(Math.abs(a - b) < eps, `${a} !≈ ${b}`);

test("normalisedRect maps a pixel box to 0-1 window coordinates", () => {
  const r = normalisedRect({ left: 480, top: 270, width: 960, height: 540 }, 1920, 1080);
  near(r.x, 0.25);
  near(r.y, 0.25);
  near(r.w, 0.5);
  near(r.h, 0.5);
});

test("a full-window box is exactly FULL", () => {
  const r = normalisedRect({ left: 0, top: 0, width: 1600, height: 900 }, 1600, 900);
  assert.ok(rectEquals(r, FULL, 1e-12));
});

test("a box hanging off the edge clamps instead of asking mpv to draw outside", () => {
  const r = normalisedRect({ left: 1800, top: 1000, width: 400, height: 400 }, 1920, 1080);
  near(r.x, 1800 / 1920);
  near(r.y, 1000 / 1080);
  near(r.w, 1 - 1800 / 1920);
  near(r.h, 1 - 1000 / 1080);
  assert.ok(r.x + r.w <= 1 + 1e-12);
  assert.ok(r.y + r.h <= 1 + 1e-12);
});

test("a box entirely offscreen collapses to zero size, never negative", () => {
  const r = normalisedRect({ left: -500, top: -500, width: 100, height: 100 }, 800, 600);
  assert.equal(r.w, 0);
  assert.equal(r.h, 0);
});

test("a degenerate window falls back to FULL rather than dividing by zero", () => {
  assert.deepEqual(normalisedRect({ left: 0, top: 0, width: 10, height: 10 }, 0, 0), FULL);
});

test("lerpRect interpolates each edge independently", () => {
  const a = { x: 0, y: 0, w: 1, h: 1 };
  const b = { x: 0.5, y: 0.6, w: 0.4, h: 0.2 };
  const m = lerpRect(a, b, 0.5);
  near(m.x, 0.25);
  near(m.y, 0.3);
  near(m.w, 0.7);
  near(m.h, 0.6);
  assert.deepEqual(lerpRect(a, b, 0), a);
  assert.deepEqual(lerpRect(a, b, 1), b);
  // Out-of-range t is clamped, so a late frame cannot overshoot the dock.
  assert.deepEqual(lerpRect(a, b, 4), b);
  assert.deepEqual(lerpRect(a, b, -4), a);
});

test("easeOutCubic is monotonic and pinned at both ends", () => {
  near(easeOutCubic(0), 0);
  near(easeOutCubic(1), 1);
  let prev = -1;
  for (let i = 0; i <= 20; i++) {
    const v = easeOutCubic(i / 20);
    assert.ok(v >= prev, `not monotonic at ${i}`);
    prev = v;
  }
  // "Out" means most of the distance is covered early.
  assert.ok(easeOutCubic(0.5) > 0.5);
});

test("rectAt lands exactly on the target and reports done", () => {
  const from = FULL;
  const to = { x: 0.7, y: 0.7, w: 0.25, h: 0.25 };
  const start = rectAt(from, to, 0, 240);
  assert.deepEqual(start.rect, from);
  assert.equal(start.done, false);

  const mid = rectAt(from, to, 120, 240);
  assert.equal(mid.done, false);
  assert.ok(mid.rect.x > 0 && mid.rect.x < to.x);
  assert.ok(mid.rect.w < 1 && mid.rect.w > to.w);

  const end = rectAt(from, to, 240, 240);
  assert.deepEqual(end.rect, to);
  assert.equal(end.done, true);

  // A frame that arrives late must not overshoot.
  assert.deepEqual(rectAt(from, to, 10_000, 240).rect, to);
});

test("a zero-duration transition snaps immediately", () => {
  const to = { x: 0.1, y: 0.1, w: 0.2, h: 0.2 };
  const r = rectAt(FULL, to, 0, 0);
  assert.deepEqual(r.rect, to);
  assert.equal(r.done, true);
});

test("undocking is the same maths run backwards", () => {
  const dock = { x: 0.72, y: 0.66, w: 0.24, h: 0.135 };
  const end = rectAt(dock, FULL, 240, 240);
  assert.deepEqual(end.rect, FULL);
});

test("rectEquals ignores sub-pixel churn but not real movement", () => {
  const a = { x: 0.5, y: 0.5, w: 0.25, h: 0.25 };
  assert.ok(rectEquals(a, { ...a, x: 0.5 + 0.0001 }));
  assert.ok(!rectEquals(a, { ...a, x: 0.51 }));
});

test("fitAspect centres 16:9 inside a taller box", () => {
  const b = fitAspect({ left: 0, top: 0, width: 400, height: 400 }, 16 / 9);
  near(b.width, 400);
  near(b.height, 225);
  near(b.top, 87.5);
  near(b.left, 0);
});

test("fitAspect centres 16:9 inside a wider box", () => {
  const b = fitAspect({ left: 10, top: 20, width: 400, height: 100 }, 16 / 9);
  near(b.height, 100);
  near(b.width, 1600 / 9);
  near(b.top, 20);
  near(b.left, 10 + (400 - 1600 / 9) / 2);
});

test("fitAspect leaves a degenerate box alone", () => {
  const box = { left: 0, top: 0, width: 0, height: 0 };
  assert.deepEqual(fitAspect(box, 16 / 9), box);
});

test("the last frame of a transition is always sent, however small the step", () => {
  const a = { x: 0, y: 0, w: 1, h: 1 };
  const b = { x: 0.00009, y: 0.00009, w: 0.9999, h: 0.9999 };
  // Mid-flight this is noise and gets skipped...
  assert.equal(shouldSend(a, b, false), false);
  // ...but on the final frame it is the difference between the video sitting
  // in its hole and sitting a hair outside it forever.
  assert.equal(shouldSend(a, b, true), true);
  // Nothing at all to do is still nothing to do.
  assert.equal(shouldSend(a, { ...a }, true), false);
});
