// The guide is a pixels-to-seconds mapping with clipping, and every way it
// can be wrong is arithmetic: a programme cut off at an edge, a zero-length
// entry, a now-line drawn for a window that does not contain now.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  GUIDE_ROW_HEIGHT,
  HOUR,
  catchupMinutes,
  clampHours,
  nowLineX,
  playability,
  programmeSpan,
  programmeStatus,
  shiftWindow,
  ticks,
  timeForX,
  windowAt,
  withinArchive,
  xForTime,
} from "../logic/guide.ts";
import { computeWindow } from "../logic/virtual.ts";

const near = (a, b, eps = 1e-9) => assert.ok(Math.abs(a - b) < eps, `${a} !≈ ${b}`);

/** Three hours wide, half-hour aligned the way windowAt() produces them, over
 *  900px of track. */
const BASE = 1_700_000_000 - (1_700_000_000 % 1800);
const WIN = { from: BASE, to: BASE + 3 * HOUR };
const W = 900;
const at = (offsetSeconds) => WIN.from + offsetSeconds;

// ------------------------------------------------------------------ window

test("the window snaps back to the half hour so the labels are round", () => {
  const w = windowAt(1_700_001_234, 3);
  assert.equal(w.from % 1800, 0);
  assert.ok(w.from <= 1_700_001_234);
  assert.ok(1_700_001_234 - w.from < 1800);
  assert.equal(w.to - w.from, 3 * HOUR);
});

test("hours are clamped to something a screen can hold", () => {
  assert.equal(clampHours(0), 1);
  assert.equal(clampHours(3), 3);
  assert.equal(clampHours(99), 6);
  assert.equal(clampHours(2.4), 2);
});

test("paging moves both edges by the same amount, and back again", () => {
  const fwd = shiftWindow(WIN, 3);
  assert.equal(fwd.from, WIN.from + 3 * HOUR);
  assert.equal(fwd.to, WIN.to + 3 * HOUR);
  assert.deepEqual(shiftWindow(fwd, -3), WIN);
});

// --------------------------------------------------------- time and pixels

test("time and pixels are inverses of each other", () => {
  for (const x of [0, 1, 123.4, 450, 899, 900]) {
    near(xForTime(timeForX(x, WIN, W), WIN, W), x, 1e-6);
  }
  near(xForTime(WIN.from, WIN, W), 0);
  near(xForTime(WIN.to, WIN, W), W);
  near(xForTime(at(1.5 * HOUR), WIN, W), W / 2);
});

test("a degenerate window or track does not divide by zero", () => {
  assert.equal(xForTime(WIN.from, { from: 5, to: 5 }, W), 0);
  assert.equal(xForTime(WIN.from, WIN, 0), 0);
  assert.equal(timeForX(10, WIN, 0), WIN.from);
});

test("ticks are the half hours inside the window and nothing else", () => {
  const t = ticks(WIN);
  assert.equal(t.length, 6);
  assert.equal(t[0], WIN.from);
  assert.ok(t.every((x) => x % 1800 === 0));
  assert.ok(t.every((x) => x >= WIN.from && x < WIN.to));
  // A window starting mid-half-hour gets its first tick later, not earlier.
  const off = { from: WIN.from + 600, to: WIN.to };
  assert.ok(ticks(off)[0] > off.from);
});

// -------------------------------------------------------------- programmes

test("a programme inside the window maps to its own slice of the track", () => {
  const p = { start: at(HOUR), stop: at(2 * HOUR) };
  const s = programmeSpan(p, WIN, W);
  near(s.left, 300);
  near(s.width, 300);
  assert.equal(s.clippedStart, false);
  assert.equal(s.clippedEnd, false);
});

test("a programme running in from before the window is clipped at the left", () => {
  const p = { start: WIN.from - HOUR, stop: at(HOUR) };
  const s = programmeSpan(p, WIN, W);
  near(s.left, 0);
  near(s.width, 300);
  assert.equal(s.clippedStart, true);
  assert.equal(s.clippedEnd, false);
});

test("a programme running out past the window is clipped at the right", () => {
  const p = { start: at(2 * HOUR), stop: WIN.to + 2 * HOUR };
  const s = programmeSpan(p, WIN, W);
  near(s.left, 600);
  near(s.width, 300);
  assert.equal(s.left + s.width, W);
  assert.equal(s.clippedEnd, true);
});

test("a programme spanning the whole window is clipped at both ends", () => {
  const s = programmeSpan({ start: WIN.from - HOUR, stop: WIN.to + HOUR }, WIN, W);
  near(s.left, 0);
  near(s.width, W);
  assert.equal(s.clippedStart, true);
  assert.equal(s.clippedEnd, true);
});

test("a programme outside the window is null, not a sliver at the edge", () => {
  assert.equal(programmeSpan({ start: WIN.from - 2 * HOUR, stop: WIN.from - HOUR }, WIN, W), null);
  assert.equal(programmeSpan({ start: WIN.to, stop: WIN.to + HOUR }, WIN, W), null);
  // Touching the edge is still outside: stop is exclusive at `from`.
  assert.equal(programmeSpan({ start: WIN.from - HOUR, stop: WIN.from }, WIN, W), null);
});

test("a zero-length programme is inside the window with no width", () => {
  const s = programmeSpan({ start: at(HOUR), stop: at(HOUR) }, WIN, W);
  assert.notEqual(s, null);
  near(s.left, 300);
  assert.equal(s.width, 0);
  assert.equal(s.clippedStart, false);
  assert.equal(s.clippedEnd, false);
});

test("a backwards programme is treated as zero-length rather than negative", () => {
  const s = programmeSpan({ start: at(HOUR), stop: at(HOUR) - 600 }, WIN, W);
  assert.ok(s === null || s.width >= 0);
});

// ---------------------------------------------------------------- now line

test("the now-line lands where the clock says", () => {
  near(nowLineX(at(1.5 * HOUR), WIN, W), 450);
  near(nowLineX(WIN.from, WIN, W), 0);
  // 20:47 on a 20:00–23:00 window, 900px wide: 47 minutes in.
  near(nowLineX(at(47 * 60), WIN, W), (47 * 60 * W) / (3 * HOUR));
});

test("the now-line is absent when now is outside the window", () => {
  assert.equal(nowLineX(WIN.from - 1, WIN, W), null);
  assert.equal(nowLineX(WIN.to, WIN, W), null);
  assert.equal(nowLineX(WIN.to + HOUR, WIN, W), null);
});

// ------------------------------------------------------------- playability

const prog = (a, b) => ({ start: at(a), stop: at(b) });

test("status is past, live or future against the clock", () => {
  assert.equal(programmeStatus(prog(0, HOUR), at(HOUR + 1)), "past");
  assert.equal(programmeStatus(prog(0, HOUR), at(30)), "live");
  assert.equal(programmeStatus(prog(HOUR, 2 * HOUR), at(30)), "future");
  // Half-open: a programme ending exactly now is over.
  assert.equal(programmeStatus(prog(0, HOUR), at(HOUR)), "past");
});

test("what a click can do: on now plays, past needs an archive", () => {
  assert.equal(playability("live", false), "play");
  assert.equal(playability("live", true), "play");
  assert.equal(playability("past", true), "catchup");
  assert.equal(playability("past", false), "inert");
  assert.equal(playability("future", true), "inert");
  assert.equal(playability("future", false), "inert");
});

test("catchup asks for whole minutes and never zero", () => {
  assert.equal(catchupMinutes(prog(0, 1800)), 30);
  assert.equal(catchupMinutes(prog(0, 3540)), 59);
  assert.equal(catchupMinutes(prog(0, 0)), 1);
  assert.equal(catchupMinutes(prog(0, 20)), 1);
});

test("an archive that does not reach back that far cannot be asked to", () => {
  const now = at(0);
  assert.equal(withinArchive({ start: now - 2 * 86400 }, now, 3), true);
  assert.equal(withinArchive({ start: now - 4 * 86400 }, now, 3), false);
  // Unknown depth is not permission to refuse.
  assert.equal(withinArchive({ start: now - 40 * 86400 }, now, null), true);
});

// ------------------------------------------------------------ virtual rows

test("the guide virtualizes on the same arithmetic as the channel list", () => {
  const total = 50_414;
  const w = computeWindow(0, 800, GUIDE_ROW_HEIGHT, total, 8);
  assert.equal(w.totalHeight, total * GUIDE_ROW_HEIGHT);
  // A screen of rows plus overscan, not sixty thousand.
  assert.ok(w.end - w.start < 50);
  const deep = computeWindow(500_000, 800, GUIDE_ROW_HEIGHT, total, 8);
  assert.equal(deep.offsetY, deep.start * GUIDE_ROW_HEIGHT);
  assert.ok(deep.start > 14_000);
});
