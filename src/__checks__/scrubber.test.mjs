// The two-zone pixel/time mapping. Property tests here, because a linear scale over 3 days would make the 4-minute buffer one pixel
// wide and the bug would be invisible rather than obvious.

import test from "node:test";
import assert from "node:assert/strict";
import {
  ARCHIVE_FRACTION,
  DAY,
  behindLiveAt,
  catchupRequest,
  geometry,
  headPixel,
  hintPixel,
  localPixel,
  pixelAtTime,
  seekCost,
  timeAtPixel,
  zoneAtPixel,
} from "../logic/scrubber.ts";

const NOW = 1_757_000_000;
const W = 800;

const archived = geometry({
  width: W,
  hasArchive: true,
  archiveDays: 3,
  bufferedBack: 240,
  now: NOW,
});

const plain = geometry({
  width: W,
  hasArchive: false,
  archiveDays: null,
  bufferedBack: 240,
  now: NOW,
});

const near = (a, b, eps = 1e-6) => assert.ok(Math.abs(a - b) < eps, `${a} !≈ ${b}`);

test("the seam sits where the archive zone ends", () => {
  near(archived.seamPx, W * ARCHIVE_FRACTION);
  assert.equal(plain.seamPx, 0);
});

test("the archive zone spans exactly the advertised days", () => {
  near(archived.archiveStart, NOW - 3 * DAY);
  near(timeAtPixel(archived, 0), NOW - 3 * DAY);
});

test("the seam is the boundary between reconnect and instant", () => {
  near(timeAtPixel(archived, archived.seamPx), NOW - 240);
  assert.equal(seekCost(archived, NOW - 241), "reconnect");
  assert.equal(seekCost(archived, NOW - 239), "instant");
  // Without catchup nothing costs a reconnect: there is nowhere to reconnect to.
  assert.equal(seekCost(plain, NOW - 100000), "instant");
});

test("the right edge is live in both layouts", () => {
  near(timeAtPixel(archived, W), NOW);
  near(timeAtPixel(plain, W), NOW);
});

test("without archive the whole bar is the buffer", () => {
  near(timeAtPixel(plain, 0), NOW - 240);
  near(timeAtPixel(plain, W / 2), NOW - 120);
  assert.equal(zoneAtPixel(plain, 0), "buffer");
  assert.equal(zoneAtPixel(plain, W - 1), "buffer");
});

test("the four-minute buffer gets a real share of the width, not one pixel", () => {
  const bufferPixels = W - archived.seamPx;
  assert.ok(bufferPixels > 200, `buffer got ${bufferPixels}px`);
  // A linear scale would have given it this, which is the bug being avoided.
  const linear = (240 / (3 * DAY)) * W;
  assert.ok(linear < 1);
});

test("pixel and time round-trip across both zones", () => {
  for (let px = 0; px <= W; px += 7) {
    near(pixelAtTime(archived, timeAtPixel(archived, px)), px, 1e-6);
    near(pixelAtTime(plain, timeAtPixel(plain, px)), px, 1e-6);
  }
});

test("time and pixel round-trip from the time side too", () => {
  const samples = [
    NOW,
    NOW - 1,
    NOW - 239,
    NOW - 240,
    NOW - 241,
    NOW - 3600,
    NOW - DAY,
    NOW - 3 * DAY,
  ];
  for (const t of samples) {
    near(timeAtPixel(archived, pixelAtTime(archived, t)), t, 1e-3);
  }
});

test("the mapping is monotonic — later time is never further left", () => {
  let prev = -Infinity;
  for (let px = 0; px <= W; px += 1) {
    const t = timeAtPixel(archived, px);
    assert.ok(t >= prev, `went backwards at ${px}`);
    prev = t;
  }
});

test("pixels outside the bar clamp to its ends", () => {
  near(timeAtPixel(archived, -500), NOW - 3 * DAY);
  near(timeAtPixel(archived, W + 500), NOW);
  near(pixelAtTime(archived, NOW + 9999), W);
  near(pixelAtTime(archived, NOW - 99 * DAY), 0);
});

test("an empty buffer puts the playhead at the live edge instead of dividing by zero", () => {
  const g = geometry({ width: W, hasArchive: false, archiveDays: null, bufferedBack: 0, now: NOW });
  near(pixelAtTime(g, NOW), W);
  near(timeAtPixel(g, 0), NOW);
  assert.ok(Number.isFinite(pixelAtTime(g, NOW - 10)));
});

test("a zero-width bar answers without NaN", () => {
  const g = geometry({ width: 0, hasArchive: true, archiveDays: 7, bufferedBack: 60, now: NOW });
  assert.equal(timeAtPixel(g, 10), NOW);
  assert.equal(pixelAtTime(g, NOW - 60), 0);
});

test("hasArchive with no archiveDays is treated as no archive", () => {
  const g = geometry({ width: W, hasArchive: true, archiveDays: null, bufferedBack: 60, now: NOW });
  assert.equal(g.hasArchive, false);
  assert.equal(g.seamPx, 0);
});

test("behindLiveAt is what seek_relative and the readout speak in", () => {
  near(behindLiveAt(archived, NOW), 0);
  near(behindLiveAt(archived, NOW - 90), 90);
  near(behindLiveAt(archived, NOW + 10), 0);
});

test("a catchup request starts on a minute boundary and runs to live", () => {
  const t = NOW - 2 * 3600 - 37;
  const r = catchupRequest(archived, t);
  assert.equal(r.start % 60, 0);
  assert.ok(r.start <= t);
  assert.ok(r.durationMinutes >= 120);
  // Clamped into the archive: asking for buffer time as catchup makes no sense.
  assert.ok(catchupRequest(archived, NOW).start <= archived.bufferStart);
});

test("a seven-day archive is seven days wide", () => {
  const g = geometry({ width: W, hasArchive: true, archiveDays: 7, bufferedBack: 240, now: NOW });
  near(timeAtPixel(g, 0), NOW - 7 * DAY);
  near(timeAtPixel(g, g.seamPx), NOW - 240);
});

test("the playhead pins to the right edge while live", () => {
  // The two things that could move it, moving. behindLive sawtooths because
  // it is measured against a seekable window that advances in chunks, and the
  // buffer zone rescales as scrollback deepens.
  const deep = geometry({ width: W, hasArchive: false, archiveDays: null, bufferedBack: 240, now: NOW });
  const shallow = geometry({ width: W, hasArchive: false, archiveDays: null, bufferedBack: 12, now: NOW });
  for (const behindLive of [0, 0.4, 1.7, 3.2]) {
    assert.equal(headPixel(deep, { atLiveEdge: true, behindLive }), W);
    assert.equal(headPixel(shallow, { atLiveEdge: true, behindLive }), W);
  }
});

test("off the live edge the playhead is plotted, because that movement is real", () => {
  const g = geometry({ width: W, hasArchive: false, archiveDays: null, bufferedBack: 240, now: NOW });
  // Half the buffer back is half way along the bar.
  near(headPixel(g, { atLiveEdge: false, behindLive: 120 }), W / 2);
  // Further back is further left, which is the whole point of not pinning.
  assert.ok(
    headPixel(g, { atLiveEdge: false, behindLive: 200 }) <
      headPixel(g, { atLiveEdge: false, behindLive: 100 }),
  );
  // A nonsense negative reads as live rather than off the right edge.
  assert.equal(headPixel(g, { atLiveEdge: false, behindLive: -5 }), W);
});

test("a pointer is converted out of visual pixels into layout pixels", () => {
  // Page zoom: the rect and clientWidth agree, so nothing is corrected.
  assert.equal(localPixel(500, 100, 800, 800), 400);
  // CSS zoom at 1.5: the element measures 1200 visual over 800 layout, so a
  // pointer 600 visual pixels along is 400 layout pixels along. Feeding the
  // uncorrected 600 into geometry built on 800 was the bug — the error grows
  // with distance, so the far end of the bar was wrong by hundreds of pixels
  // and a click there seeked somewhere else entirely.
  assert.equal(localPixel(700, 100, 1200, 800), 400);
  // Before layout there is nothing to scale by, and the raw offset is the
  // only honest answer.
  assert.equal(localPixel(500, 100, 0, 800), 400);
  assert.equal(localPixel(500, 100, 800, 0), 400);
});

test("the hover tooltip is kept inside the bar", () => {
  // Centred on the pointer where there is room for it.
  assert.equal(hintPixel(400, 200, 800), 400);
  // Within half its width of an end it stops sliding, so the cursor walks
  // along it instead of dragging it off the screen.
  assert.equal(hintPixel(10, 200, 800), 100);
  assert.equal(hintPixel(790, 200, 800), 700);
  // Exactly at the limit it is untouched.
  assert.equal(hintPixel(100, 200, 800), 100);
  assert.equal(hintPixel(700, 200, 800), 700);
});

test("a tooltip too wide for the bar spreads its overhang evenly", () => {
  // It cannot be placed without overhang, so it is centred rather than
  // pushed entirely off one side.
  assert.equal(hintPixel(0, 900, 800), 400);
  assert.equal(hintPixel(800, 900, 800), 400);
  // Nothing measured yet: leave it where it was asked for.
  assert.equal(hintPixel(123, 0, 800), 123);
  assert.equal(hintPixel(123, 200, 0), 123);
});
