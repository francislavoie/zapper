// The diagnostics panel's arithmetic.
//
// Worth testing because the graphs are the part people will trust, and a
// graph that is subtly wrong is worse than no graph: it makes a healthy
// stream look broken, or hides the stutter somebody opened the panel to
// find.

import test from "node:test";
import assert from "node:assert/strict";
import { span } from "../logic/format.ts";
import {
  HISTORY_LENGTH,
  NONE,
  bitrate,
  bytes,
  count,
  rate,
  resolutionLine,
  sampleAt,
  sampleX,
  seconds,
  sparkline,
  text,
} from "../logic/stats.ts";

test("the hover cursor finds the sample under the pointer, both ways round", () => {
  const n = HISTORY_LENGTH;
  // A full graph: the left edge is the oldest sample, the right the newest.
  assert.equal(sampleAt(0, n), 0);
  assert.equal(sampleAt(1, n), n - 1);
  assert.equal(sampleAt(0.5, n), Math.round((n - 1) / 2));
  // Round trip: the x a sample is drawn at points back at that sample.
  for (const i of [0, 7, 60, n - 1]) assert.equal(sampleAt(sampleX(i, n), n), i);
  // A graph with ten samples is drawn from the right; its empty left side
  // points at nothing rather than at the oldest sample.
  assert.equal(sampleAt(0.1, 10), null);
  assert.equal(sampleAt(1, 10), 9);
  assert.equal(sampleAt(sampleX(0, 10), 10), 0);
  // Nothing to point at.
  assert.equal(sampleAt(0.5, 0), null);
  assert.equal(sampleAt(Number.NaN, 10), null);
});

// The sampling itself — the delta arithmetic, the ring, the reset on a new
// stream — lives in the backend, which records continuously so the panel
// opens onto a full graph rather than starting one. It is tested there, in
// src-tauri/src/ipc/health.rs. What is left on this side is drawing.

test("the sparkline puts the newest sample on the right edge", () => {
  // A short history is drawn at the right-hand end rather than stretched,
  // so a graph that has just started does not rescale on every tick.
  const path = sparkline([0, 10], 100, 20, 10);
  const points = path.split(" ").map((p) => p.slice(1).split(",").map(Number));
  assert.equal(points.length, 2);
  assert.ok(Math.abs(points[1][0] - 100) < 0.05, `last x was ${points[1][0]}`);
  // Full scale is the top of the box, which is y=0.
  assert.equal(points[1][1], 0);
  // Zero is the bottom.
  assert.equal(points[0][1], 20);
});

test("the sparkline survives the states it is actually in", () => {
  assert.equal(sparkline([], 100, 20), "");
  assert.equal(sparkline([1], 0, 20), "");
  assert.equal(sparkline([1], 100, 0), "");
  // All zeros must not divide by zero.
  assert.ok(sparkline([0, 0, 0], 100, 20).startsWith("M"));
  // A value above the ceiling is clamped into the box rather than drawn
  // outside it, where it would simply vanish.
  const over = sparkline([100], 100, 20, 10);
  assert.ok(over.endsWith(",0.0"), over);
});

test("a missing reading is a dash, not a zero", () => {
  // "0 Mbps" is a claim about the stream; a dash says the stream does not
  // report it, which is the truth for most of these on MPEG-TS.
  for (const f of [bitrate, bytes, rate, seconds, count]) {
    assert.equal(f(null), NONE, f.name);
    assert.equal(f(undefined), NONE, f.name);
  }
  assert.equal(text(null), NONE);
  assert.equal(text(""), NONE);
  assert.equal(bitrate(0), NONE);
  assert.equal(bitrate(Number.NaN), NONE);
});

test("rates and sizes read the way people say them", () => {
  assert.equal(bitrate(8_200_000), "8.20 Mbps");
  assert.equal(bitrate(128_000), "128 kbps");
  assert.equal(bytes(1536), "2 KB");
  assert.equal(bytes(5 * 1024 ** 2), "5.0 MB");
  assert.equal(rate(1_048_576), "1.0 MB/s");
  assert.equal(seconds(14.567, 1), "14.6s");
  assert.equal(count(285_728), (285728).toLocaleString());
});

test("the resolution line shows the measured rate beside the declared one", () => {
  // Which is the readout that says frames are being dropped.
  assert.equal(resolutionLine(1920, 1080, 50, 49.8), "1920×1080@50 (49.8 fps measured)");
  assert.equal(resolutionLine(1920, 1080, 59.94, null), "1920×1080@59.94");
  assert.equal(resolutionLine(1280, 720, null, null), "1280×720");
  // Before anything has been measured.
  assert.equal(resolutionLine(null, null, 50, 50), NONE);
});

test("span keeps decimals under a minute and says minutes past it", () => {
  assert.equal(span(4.24, 1), "4.2s");
  assert.equal(span(640, 1), "10m40s");
  assert.equal(span(null), NONE);
});
