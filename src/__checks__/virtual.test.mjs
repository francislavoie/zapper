// Windowing arithmetic at a realistic scale. If any of this is wrong the list
// either renders 60,000 rows and hangs, or renders the wrong ones.

import test from "node:test";
import assert from "node:assert/strict";
import {
  ROW_HEIGHT,
  computeWindow,
  missingPages,
  pageOf,
  pagesForWindow,
  projectedTotal,
  stickyIndex,
} from "../logic/virtual.ts";

const TOTAL = 60000;

test("an unscrolled window starts at zero and covers the viewport", () => {
  const w = computeWindow(0, 1000, 26, TOTAL, 8);
  assert.equal(w.start, 0);
  // 1000px of 26px rows is 39 visible rows (39*26 = 1014 covers it).
  assert.equal(w.end, 39 + 8);
  assert.equal(w.offsetY, 0);
  assert.equal(w.totalHeight, TOTAL * 26);
});

test("the rendered count stays tiny however far down we are", () => {
  for (const top of [0, 26, 1000, 500_000, TOTAL * 26 - 1000]) {
    const w = computeWindow(top, 1000, 26, TOTAL, 8);
    assert.ok(w.end - w.start <= 40 + 16, `too many rows at ${top}`);
    assert.ok(w.end <= TOTAL);
    assert.ok(w.start >= 0);
  }
});

test("offsetY always positions the first rendered row exactly", () => {
  for (const top of [0, 13, 26, 27, 12_345, 999_999]) {
    const w = computeWindow(top, 800, 26, TOTAL, 8);
    assert.equal(w.offsetY, w.start * 26);
  }
});

test("the window always contains every visible row", () => {
  for (const top of [0, 5, 26, 260, 7777, 1_310_000]) {
    const w = computeWindow(top, 900, 26, TOTAL, 8);
    const firstVisible = Math.floor(top / 26);
    const lastVisible = Math.floor((top + 900 - 1) / 26);
    assert.ok(w.start <= firstVisible, `missed top at ${top}`);
    assert.ok(w.end >= Math.min(TOTAL, lastVisible + 1), `missed bottom at ${top}`);
  }
});

test("scrolling past the end clamps instead of rendering past the data", () => {
  const w = computeWindow(99_999_999, 1000, 26, TOTAL, 8);
  assert.equal(w.end, TOTAL);
  assert.ok(w.start < w.end);
  assert.ok(w.start >= TOTAL - 40 - 16);
});

test("a negative scrollTop behaves as the top", () => {
  const w = computeWindow(-500, 1000, 26, TOTAL, 8);
  assert.equal(w.start, 0);
  assert.equal(w.offsetY, 0);
});

test("an empty list renders nothing and has no height", () => {
  const w = computeWindow(0, 1000, 26, 0, 8);
  assert.deepEqual(w, { start: 0, end: 0, offsetY: 0, totalHeight: 0 });
});

test("a viewport of zero height still yields a valid empty-ish window", () => {
  const w = computeWindow(0, 0, 26, TOTAL, 0);
  assert.ok(w.end >= w.start);
  assert.ok(w.end - w.start <= 1);
});

test("a single row is fully rendered", () => {
  const w = computeWindow(0, 1000, 26, 1, 8);
  assert.equal(w.start, 0);
  assert.equal(w.end, 1);
  assert.equal(w.totalHeight, 26);
});

test("row height is dense enough for roughly forty rows", () => {
  const rows = Math.floor(1040 / ROW_HEIGHT);
  assert.ok(rows >= 38 && rows <= 44, `got ${rows} rows per 1040px`);
});

test("pageOf and pagesForWindow agree on boundaries", () => {
  assert.equal(pageOf(0, 200), 0);
  assert.equal(pageOf(199, 200), 0);
  assert.equal(pageOf(200, 200), 1);
  assert.deepEqual(pagesForWindow(0, 200, 200), [0]);
  assert.deepEqual(pagesForWindow(0, 201, 200), [0, 1]);
  assert.deepEqual(pagesForWindow(199, 401, 200), [0, 1, 2]);
  assert.deepEqual(pagesForWindow(5, 5, 200), []);
  assert.deepEqual(pagesForWindow(400, 402, 200), [2]);
});

test("missingPages skips what is already loaded or in flight", () => {
  const have = new Set([0, 2]);
  assert.deepEqual(missingPages(0, 601, 200, (p) => have.has(p)), [1, 3]);
  assert.deepEqual(missingPages(0, 400, 200, (p) => have.has(p)), [1]);
  assert.deepEqual(missingPages(0, 200, 200, (p) => have.has(p)), []);
});

test("the total grows while unproven and freezes once a short page proves it", () => {
  // channelCount excludes separators, so the estimate is a floor.
  assert.equal(projectedTotal(1000, 0, 200, null), 1000);
  assert.equal(projectedTotal(1000, 1000, 200, null), 1200);
  assert.equal(projectedTotal(1000, 1200, 200, null), 1400);
  // A short page is proof, and proof wins even when it is smaller than the
  // estimate — a category can shrink between sync and scroll.
  assert.equal(projectedTotal(1000, 1200, 200, 1043), 1043);
  assert.equal(projectedTotal(1000, 1200, 200, 0), 0);
});

test("the sticky header is the nearest separator at or above the first row", () => {
  const seps = new Set([0, 61, 122]);
  const at = (i) => seps.has(i);
  assert.equal(stickyIndex(at, 0), 0);
  assert.equal(stickyIndex(at, 60), 0);
  assert.equal(stickyIndex(at, 61), 61);
  assert.equal(stickyIndex(at, 121), 61);
  assert.equal(stickyIndex(at, 200), 122);
});

test("no header is claimed when the rows above are not loaded", () => {
  const at = (i) => (i > 500 ? false : undefined);
  assert.equal(stickyIndex(at, 600), null);
});

test("no header before the first separator", () => {
  assert.equal(stickyIndex(() => false, 30), null);
});

test("the sticky scan is bounded rather than walking 50k rows", () => {
  let visits = 0;
  stickyIndex(() => {
    visits++;
    return false;
  }, 50_000, 400);
  assert.ok(visits <= 401, `scanned ${visits} rows`);
});
