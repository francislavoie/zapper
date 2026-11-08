// The docked picture sits in the top right corner of the window, and "where
// is the video" is arithmetic — the same arithmetic the hook runs on the
// measured slot. Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import { FULL, fitAspect, normalisedRect, rectEquals } from "../logic/rect.ts";
import { laterOn, splitNowNext } from "../logic/guide.ts";
import { playerBadge } from "../logic/format.ts";

const near = (a, b, eps = 1e-9) => assert.ok(Math.abs(a - b) < eps, `${a} !≈ ${b}`);

/** The slot as CSS lays it out: --video-w wide, 16:9, flush with the top right
 *  corner. Everything else in browse mode is placed around it. */
const slot = (winW, winH, videoW) => {
  const box = { left: winW - videoW, top: 0, width: videoW, height: (videoW * 9) / 16 };
  return normalisedRect(fitAspect(box, 16 / 9), winW, winH);
};

test("the docked picture is pinned to the top right corner", () => {
  const r = slot(1600, 900, 356);
  near(r.x, (1600 - 356) / 1600);
  near(r.y, 0);
  near(r.w, 356 / 1600);
  near(r.h, ((356 * 9) / 16) / 900);
  // Flush with two edges, and only two.
  near(r.x + r.w, 1);
  near(r.y, 0);
  assert.ok(r.y + r.h < 1, "the list gets everything below the band");
});

test("the picture keeps 16:9 and its corner at any window size", () => {
  for (const [w, h, videoW] of [
    [1600, 900, 356],
    [1280, 720, 356],
    [2560, 1440, 356],
    [1024, 640, 300],
  ]) {
    const r = slot(w, h, videoW);
    near(r.x + r.w, 1);
    near(r.y, 0);
    near(r.w * w, videoW);
    // 16:9 back out of window coordinates.
    near((r.w * w) / (r.h * h), 16 / 9, 1e-9);
  }
});

test("a slot taller than 16:9 keeps its width and stays at the top", () => {
  // Defensive: if `aspect-ratio` ever fails to apply, fitAspect is what stops
  // the picture from stretching.
  const fitted = fitAspect({ left: 1244, top: 0, width: 356, height: 400 }, 16 / 9);
  near(fitted.width, 356);
  near(fitted.height, (356 * 9) / 16);
  near(fitted.left, 1244);
});

test("watch mode is the whole window", () => {
  assert.ok(
    rectEquals(normalisedRect({ left: 0, top: 0, width: 1600, height: 900 }, 1600, 900), FULL, 1e-12),
  );
});

test("a window narrower than the picture clamps instead of drawing outside", () => {
  const r = slot(300, 200, 356);
  assert.ok(r.x >= 0 && r.x + r.w <= 1 + 1e-12);
  assert.ok(r.h > 0);
});

// -------------------------------------------------------------- now / next

const prog = (start, stop, title) => ({ start, stop, title, description: null });

test("splitNowNext picks the programme on air and the one after it", () => {
  const list = [prog(0, 100, "a"), prog(100, 200, "b"), prog(200, 300, "c")];
  const s = splitNowNext(list, 150);
  assert.equal(s.now.title, "b");
  assert.equal(s.next.title, "c");
});

test("an unsorted list still resolves, because XMLTV order is not a promise", () => {
  const list = [prog(200, 300, "c"), prog(0, 100, "a"), prog(100, 200, "b")];
  const s = splitNowNext(list, 150);
  assert.equal(s.now.title, "b");
  assert.equal(s.next.title, "c");
});

test("no programmes at all is not an error — it is most channels", () => {
  assert.deepEqual(splitNowNext([], 150), { now: null, next: null });
});

test("a gap in the schedule gives a next but no now", () => {
  const s = splitNowNext([prog(0, 100, "a"), prog(200, 300, "c")], 150);
  assert.equal(s.now, null);
  assert.equal(s.next.title, "c");
});

test("the last programme of the day has no next", () => {
  const s = splitNowNext([prog(0, 100, "a")], 50);
  assert.equal(s.now.title, "a");
  assert.equal(s.next, null);
});

test("boundaries are half-open: stop belongs to the next programme", () => {
  const list = [prog(0, 100, "a"), prog(100, 200, "b")];
  assert.equal(splitNowNext(list, 100).now.title, "b");
  assert.equal(splitNowNext(list, 99).now.title, "a");
});

test("overlapping entries hand the slot to the later start", () => {
  const list = [prog(0, 200, "old"), prog(100, 200, "correction")];
  assert.equal(splitNowNext(list, 150).now.title, "correction");
});

// ----------------------------------------------------------- measured badge

const player = (p) => ({
  state: "playing",
  streamId: 1,
  width: null,
  height: null,
  fps: null,
  codec: null,
  bufferedBack: 0,
  behindLive: 0,
  volume: 100,
  muted: false,
  compressor: false,
  error: null,
  ...p,
});

test("the band's badge reports measured height, not the channel's claim", () => {
  assert.equal(playerBadge(player({ width: 1280, height: 720 })), "720p");
  assert.equal(playerBadge(player({ width: 3840, height: 2160 })), "4K");
});

test("nothing measured means no badge, not a wrong one", () => {
  assert.equal(playerBadge(player({})), null);
});

test("laterOn lists what starts after now, in order, as many as asked", () => {
  const p = (start, title) => ({ start, stop: start + 30, title });
  const list = [p(300, "c"), p(100, "on now"), p(200, "b"), p(150, "a"), p(400, "d")];
  assert.deepEqual(laterOn(list, 120, 3).map((x) => x.title), ["a", "b", "c"]);
  assert.deepEqual(laterOn(list, 500, 3), []);
});
