// The guide is a pixels-to-seconds mapping with clipping, and every way it
// can be wrong is arithmetic: a programme cut off at an edge, a zero-length
// entry, a now-line drawn for a window that does not contain now.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  GUIDE_CHUNK,
  GUIDE_FUTURE_HOURS,
  GUIDE_PAST_HOURS,
  GUIDE_REACH,
  GUIDE_ROW_HEIGHT,
  GUIDE_THUMB_MIN_PX,
  HOUR,
  anchorFor,
  catchupMinutes,
  chunksFor,
  clampHours,
  clampOffset,
  dayLabel,
  dragOffset,
  easeToward,
  guideWindow,
  horizontalPixels,
  nowLineX,
  pageDirection,
  playability,
  programmeSpan,
  programmeStatus,
  rebaseOffset,
  scrollThumb,
  scrollbarX,
  secondsForPixels,
  shiftWindow,
  showsNow,
  ticks,
  timeForX,
  visibleProgrammes,
  windowAt,
  withinArchive,
  xForTime,
} from "../logic/guide.ts";
import { computeWindow } from "../logic/virtual.ts";
import { takesWheel } from "../logic/wheel.ts";

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

// ------------------------------------------------------- scrolling in time

test("an offset stays inside the guide's reach, for a full last screen", () => {
  assert.equal(clampOffset(0, 3), 0);
  assert.equal(clampOffset(-5 * HOUR, 3), -5 * HOUR);
  assert.equal(clampOffset(-100 * HOUR, 3), -GUIDE_PAST_HOURS * HOUR);
  // The far edge clamps the window's right side: the last screen ends at the
  // reach, rather than starting there and running on into nothing.
  assert.equal(clampOffset(100 * HOUR, 3), (GUIDE_FUTURE_HOURS - 3) * HOUR);
  assert.equal(clampOffset(100 * HOUR, 6), (GUIDE_FUTURE_HOURS - 6) * HOUR);
  // Not rounded: a hundred one-pixel touchpad events must add up to exactly
  // a hundred pixels, at 9.15 seconds a pixel as much as at 12.
  let off = 0;
  for (let i = 0; i < 100; i++) off = clampOffset(off + secondsForPixels(-1, 3, 1180), 3);
  near(off, secondsForPixels(-100, 3, 1180), 1e-6);
});

test("the reach goes back at least a day, which is what catchup is for", () => {
  assert.ok(GUIDE_PAST_HOURS >= 24);
  assert.ok(GUIDE_FUTURE_HOURS >= 24);
  const now = 1_700_001_234;
  const back = guideWindow(now, 3, -1e9);
  assert.ok(back.from <= now - 24 * HOUR + 1800);
  const fwd = guideWindow(now, 3, 1e9);
  assert.equal(fwd.to, anchorFor(now) + GUIDE_FUTURE_HOURS * HOUR);
});

test("an offset of zero is the window windowAt() gives", () => {
  const now = 1_700_001_234;
  assert.deepEqual(guideWindow(now, 3, 0), windowAt(now, 3));
  assert.equal(anchorFor(now), windowAt(now, 3).from);
  const w = guideWindow(now, 3, -600);
  assert.equal(w.from, windowAt(now, 3).from - 600);
  assert.equal(w.to - w.from, 3 * HOUR);
});

test("a scrolled guide stays put when the clock passes the half hour", () => {
  const before = 1_700_001_234;
  const after = before + 1800;
  const a = anchorFor(before);
  const b = anchorFor(after);
  assert.equal(b - a, 1800);
  const off = rebaseOffset(-2 * HOUR, a, b, 3);
  assert.equal(guideWindow(after, 3, off).from, guideWindow(before, 3, -2 * HOUR).from);
  // Following now keeps following.
  assert.equal(rebaseOffset(0, a, b, 3), 0);
  // No earlier anchor, nothing to correct.
  assert.equal(rebaseOffset(-HOUR, null, b, 3), -HOUR);
  // Carried past the reach, it stops at the reach.
  assert.equal(rebaseOffset(-GUIDE_PAST_HOURS * HOUR, a, b, 3), -GUIDE_PAST_HOURS * HOUR);
});

test("the Now button lights for any window with now in it", () => {
  const now = 1_700_001_234;
  assert.equal(showsNow(now, guideWindow(now, 3, 0)), true);
  assert.equal(showsNow(now, guideWindow(now, 3, -600)), true);
  assert.equal(showsNow(now, guideWindow(now, 3, -3 * HOUR)), false);
  assert.equal(showsNow(now, guideWindow(now, 3, 3 * HOUR)), false);
});

const wheel = (o) => ({
  deltaX: 0,
  deltaY: 0,
  deltaMode: 0,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  ...o,
});

test("sideways is a swipe, a tilt wheel, or Shift with the wheel", () => {
  // Touchpad swipe or tilt wheel.
  assert.equal(horizontalPixels(wheel({ deltaX: -30 }), 900), -30);
  assert.equal(horizontalPixels(wheel({ deltaX: 12, deltaY: 3 }), 900), 12);
  // Shift+wheel, whether or not the engine has already turned it sideways.
  assert.equal(horizontalPixels(wheel({ shiftKey: true, deltaY: -53 }), 900), -53);
  assert.equal(horizontalPixels(wheel({ shiftKey: true, deltaX: -53 }), 900), -53);
  // Lines and pages.
  assert.equal(horizontalPixels(wheel({ deltaX: -3, deltaMode: 1 }), 900), -120);
  assert.equal(horizontalPixels(wheel({ deltaX: 1, deltaMode: 2 }), 900), 900);
});

test("vertical and zoom gestures are not the guide's to take", () => {
  assert.equal(horizontalPixels(wheel({ deltaY: 53 }), 900), null);
  // A diagonal that is mostly vertical belongs to the rows.
  assert.equal(horizontalPixels(wheel({ deltaX: 5, deltaY: 20 }), 900), null);
  // An exact tie goes vertical, as logic/wheel.ts takes it.
  assert.equal(horizontalPixels(wheel({ deltaX: 10, deltaY: 10 }), 900), null);
  assert.equal(horizontalPixels(wheel({ deltaX: 40, ctrlKey: true }), 900), null);
  assert.equal(horizontalPixels(wheel({ deltaX: 40, metaKey: true }), 900), null);
  assert.equal(horizontalPixels(wheel({ shiftKey: true }), 900), null);
});

test("the page's vertical wheel handling and the guide never both take one event", () => {
  const cases = [
    { deltaY: 53 },
    { deltaX: 53 },
    { deltaX: 5, deltaY: 20 },
    { deltaX: 20, deltaY: 5 },
    { deltaX: 10, deltaY: 10 },
    { shiftKey: true, deltaY: 53 },
    { shiftKey: true, deltaX: 53 },
    { ctrlKey: true, deltaY: 53 },
  ];
  for (const c of cases) {
    const e = { ...wheel(c), defaultPrevented: false };
    const both = takesWheel(e) && horizontalPixels(e, 900) !== null;
    assert.equal(both, false, JSON.stringify(c));
  }
});

test("a swipe moves the axis exactly as far as the fingers went", () => {
  // 3h over 900px: 12 seconds a pixel.
  assert.equal(secondsForPixels(1, 3, 900), 12);
  assert.equal(secondsForPixels(-450, 3, 900), -1.5 * HOUR);
  assert.equal(secondsForPixels(10, 3, 0), 0);
  // What was under x is under x + 200 after the content is dragged 200px
  // right (a swipe back in time).
  const now = 1_700_001_234;
  const win = guideWindow(now, 3, 0);
  const t = timeForX(600, win, 900);
  const moved = guideWindow(now, 3, secondsForPixels(-200, 3, 900));
  near(xForTime(t, moved, 900), 800, 1e-6);
});

// ------------------------------------------------------- fetching in chunks

test("data is asked for in clock-aligned chunks, not per window", () => {
  const c = chunksFor(WIN);
  assert.ok(c.length >= 1 && c.length <= 2);
  assert.ok(c.every((x) => x % GUIDE_CHUNK === 0));
  assert.ok(c[0] <= WIN.from && c[c.length - 1] + GUIDE_CHUNK >= WIN.to);
  // A pixel's scroll inside a chunk asks nothing new.
  const nudged = { from: WIN.from + 12, to: WIN.to + 12 };
  assert.deepEqual(chunksFor(nudged), c);
});

test("the margin fetches the neighbouring chunks before they are reached", () => {
  const from = 10 * GUIDE_CHUNK + GUIDE_CHUNK - HOUR; // an hour before a boundary
  const win = { from, to: from + 3 * HOUR };
  const plain = chunksFor(win);
  assert.deepEqual(plain, [10 * GUIDE_CHUNK, 11 * GUIDE_CHUNK]);
  const ahead = chunksFor(win, 1.5 * HOUR);
  assert.ok(plain.every((c) => ahead.includes(c)));
  // A window right at a chunk's start reaches back into the one before.
  const edge = { from: 10 * GUIDE_CHUNK, to: 10 * GUIDE_CHUNK + 3 * HOUR };
  assert.deepEqual(chunksFor(edge), [10 * GUIDE_CHUNK]);
  assert.deepEqual(chunksFor(edge, HOUR), [9 * GUIDE_CHUNK, 10 * GUIDE_CHUNK]);
  // At the widest setting, at most three chunks with its margin.
  const wide = { from: 10 * GUIDE_CHUNK + 1, to: 10 * GUIDE_CHUNK + 1 + 6 * HOUR };
  assert.ok(chunksFor(wide, 3 * HOUR).length <= 3);
  assert.deepEqual(chunksFor({ from: 5, to: 5 }), []);
});

test("a row's programmes come out once each, in order, only those on screen", () => {
  const p = (a, b, title = "x") => ({ start: at(a), stop: at(b), title, description: null });
  // The 1h–2h programme straddles a chunk boundary and comes back twice.
  const first = [p(-HOUR, 0), p(0, HOUR), p(HOUR, 2 * HOUR, "a")];
  const second = [p(2 * HOUR, 3 * HOUR), p(HOUR, 2 * HOUR, "b"), p(3 * HOUR, 4 * HOUR)];
  const got = visibleProgrammes([second, first].reverse(), WIN);
  assert.deepEqual(
    got.map((x) => x.start),
    [at(0), at(HOUR), at(2 * HOUR)],
  );
  assert.equal(got[1].title, "a");
  assert.deepEqual(visibleProgrammes([], WIN), []);
});

// ------------------------------------------------------------- the header

test("the header names the day once the window leaves today", () => {
  const noon = new Date(2026, 9, 31, 12, 0, 0).getTime() / 1000;
  assert.equal(dayLabel(noon, noon), "");
  assert.equal(dayLabel(noon - 11 * HOUR, noon), "");
  assert.equal(dayLabel(noon - 13 * HOUR, noon), "Yesterday");
  assert.equal(dayLabel(noon + 13 * HOUR, noon), "Tomorrow");
  assert.notEqual(dayLabel(noon + 49 * HOUR, noon), "");
  assert.notEqual(dayLabel(noon + 49 * HOUR, noon), "Tomorrow");
});

// ------------------------------------------------------------- easing

test("easing closes on the target and lands on it exactly", () => {
  // Ten notches' worth, as one target, from a standing start.
  const target = 10 * 760;
  let x = 0;
  let frames = 0;
  while (x !== target && frames < 200) {
    const next = easeToward(x, target, 16, 70, 1);
    assert.ok(next > x, "always moves toward the target");
    assert.ok(next <= target, "never overshoots");
    x = next;
    frames++;
  }
  assert.equal(x, target);
  // Exponential: most of the way there in a few frames, done well within a
  // second at 60 fps.
  assert.ok(frames < 60, `took ${frames} frames`);
  assert.ok(easeToward(0, target, 3 * 70, 70, 1) > 0.94 * target);
});

test("easing works backwards, snaps when close, and never steps back in time", () => {
  assert.ok(easeToward(1000, 0, 16) < 1000);
  assert.equal(easeToward(100.4, 100, 16, 70, 1), 100);
  assert.equal(easeToward(5, 5, 16), 5);
  // A frame stamp from before the animation started is no time, not a step
  // the wrong way.
  assert.equal(easeToward(0, 1000, -8, 70, 1), 0);
  // A long stall (a hidden tab) arrives rather than overshooting.
  assert.equal(easeToward(0, 1000, 10_000, 70, 1), 1000);
});

test("a target that moves mid-animation is chased, not restarted", () => {
  // Two notches 30ms apart: the second adds to where the target is going,
  // and the drawn value ends at the sum.
  let x = 0;
  let target = 760;
  for (let t = 0; t < 30; t += 10) x = easeToward(x, target, 10);
  target += 760;
  for (let i = 0; i < 100; i++) x = easeToward(x, target, 16);
  assert.equal(x, 2 * 760);
});

// ---------------------------------------------------------- the scrollbar

test("the thumb is the window's share of the reach, at the window's place", () => {
  assert.equal(GUIDE_REACH, 72 * HOUR);
  const track = 720;
  // 3h of 72h is 30px of 720: above the floor, so exact.
  const at0 = scrollThumb(0, 3, track);
  assert.equal(at0.width, 30);
  // Following now, the window starts 24h into the reach.
  assert.equal(at0.left, 240);
  // The ends of the reach are the ends of the track.
  assert.equal(scrollThumb(-GUIDE_PAST_HOURS * HOUR, 3, track).left, 0);
  const end = scrollThumb(Infinity, 3, track);
  assert.equal(end.left + end.width, track);
  // Out of range is clamped, like the offset itself.
  assert.equal(scrollThumb(-1e9, 3, track).left, 0);
});

test("a narrow window still gets a thumb a pointer can hold", () => {
  const track = 720;
  const t = scrollThumb(0, 1, track);
  assert.equal(t.width, GUIDE_THUMB_MIN_PX);
  const end = scrollThumb(Infinity, 1, track);
  assert.ok(Math.abs(end.left + end.width - track) < 1e-9);
  assert.deepEqual(scrollThumb(0, 3, 0), { left: 0, width: 0 });
});

test("dragging the thumb moves the window one to one with the pointer", () => {
  const track = 720;
  for (const hours of [1, 3, 6]) {
    const start = -2 * HOUR;
    const off = dragOffset(start, 50, hours, track);
    const moved = scrollThumb(off, hours, track).left - scrollThumb(start, hours, track).left;
    assert.ok(Math.abs(moved - 50) < 1e-9, `${hours}h moved ${moved}`);
  }
  // Clamped at both ends.
  assert.equal(dragOffset(0, -1e6, 3, track), -GUIDE_PAST_HOURS * HOUR);
  assert.equal(dragOffset(0, 1e6, 3, track), clampOffset(Infinity, 3));
  // No track, no movement — and no division by zero.
  assert.equal(dragOffset(HOUR, 50, 3, 0), HOUR);
});

test("the now mark sits inside the thumb while the window shows now", () => {
  const track = 720;
  const anchor = anchorFor(1_800_000_000);
  for (const hours of [1, 3, 6]) {
    const now = anchor + 600;
    const thumb = scrollThumb(0, hours, track);
    const x = scrollbarX(now, anchor, hours, track);
    assert.ok(x >= thumb.left && x <= thumb.left + thumb.width, `${hours}h: ${x}`);
    // A window centred on an instant puts the mark at the thumb's centre.
    const centred = scrollThumb(now - anchor - (hours * HOUR) / 2, hours, track);
    assert.ok(Math.abs(x - (centred.left + centred.width / 2)) < 1e-9);
  }
  // Without the floor, the mapping is plain proportion over the reach.
  const t = anchor + 6 * HOUR;
  assert.ok(Math.abs(scrollbarX(t, anchor, 3, 720) - ((24 + 6) / 72) * 720) < 1e-9);
});

test("a click beside the thumb pages that way", () => {
  const thumb = { left: 100, width: 30 };
  assert.equal(pageDirection(10, thumb), -1);
  assert.equal(pageDirection(115, thumb), 0);
  assert.equal(pageDirection(200, thumb), 1);
});
