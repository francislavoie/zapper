import test from "node:test";
import assert from "node:assert/strict";
import {
  CAPTURE_WINDOW_S,
  TITLE_MAX_SLOTS,
  TITLE_SPACING_S,
  captureDue,
  keepFrame,
  letterbox,
  slotOf,
  titleFrameAt,
  titleSpacing,
} from "../logic/trickplay.ts";

test("a title is ten seconds a slot unless that is too many, as the app spaces it", () => {
  assert.equal(TITLE_SPACING_S, 10);
  assert.equal(titleSpacing(null), 10);
  assert.equal(titleSpacing(Number.NaN), 10);
  assert.equal(titleSpacing(90 * 60), 10);
  assert.equal(titleSpacing(3 * 3600), 10);
  assert.equal(titleSpacing(4 * 3600), 15);
  const ten = titleSpacing(10 * 3600);
  assert.equal(ten % 5, 0);
  assert.ok((10 * 3600) / ten <= TITLE_MAX_SLOTS);
});

test("a moment belongs to the nearest slot", () => {
  assert.equal(slotOf(0, 10), 0);
  assert.equal(slotOf(4.9, 10), 0);
  assert.equal(slotOf(5.1, 10), 1);
  assert.equal(slotOf(54, 10), 5);
});

test("a played frame is taken once per slot, close to the slot's moment", () => {
  const none = new Map();
  assert.equal(captureDue(none, 49.8, 10), 5);
  assert.equal(captureDue(none, 50 + CAPTURE_WINDOW_S + 0.1, 10), null, "too far past the slot's moment");
  assert.equal(captureDue(none, 46, 10), null, "not there yet");
  assert.equal(captureDue(new Map([[5, "x"]]), 50.2, 10), null, "already has one");
  assert.equal(captureDue(none, Number.NaN, 10), null);
  assert.equal(captureDue(none, -0.2, 10), 0);
});

test("the frames kept for a title are bounded", () => {
  let frames = new Map();
  for (let k = 0; k < TITLE_MAX_SLOTS + 20; k++) frames = keepFrame(frames, k, `f${k}`);
  assert.equal(frames.size, TITLE_MAX_SLOTS);
  assert.equal(keepFrame(frames, 5, "again").get(5), "f5", "the first of a slot stays");
});

test("the bar's preview is the frame of the slot under the pointer, or none", () => {
  const frames = new Map([[5, "five"]]);
  assert.equal(titleFrameAt(frames, 54, 10), "five");
  assert.equal(titleFrameAt(frames, 45.5, 10), "five");
  assert.equal(titleFrameAt(frames, 55.5, 10), null);
  assert.equal(titleFrameAt(frames, Number.NaN, 10), null);
});

test("a played frame keeps its shape inside the preview, as the app's do", () => {
  assert.deepEqual(letterbox(1920, 1080, 160, 90), { x: 0, y: 0, width: 160, height: 90 });
  // 4:3 is pillarboxed, 2.39:1 letterboxed, both centred.
  assert.deepEqual(letterbox(1440, 1080, 160, 90), { x: 20, y: 0, width: 120, height: 90 });
  const wide = letterbox(1920, 804, 160, 90);
  assert.equal(wide.width, 160);
  assert.equal(wide.height, 67);
  assert.equal(wide.y, 11);
  assert.deepEqual(letterbox(0, 0, 160, 90), { x: 0, y: 0, width: 160, height: 90 });
});
