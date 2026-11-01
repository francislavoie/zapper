// The control bar's arithmetic: where the volume handle sits, what a point on
// the track means, and when the stream counts as live. None of it needs eyes.
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  UNITY_FRACTION,
  UNITY_SNAP,
  VOLUME_MAX,
  VOLUME_STEP,
  VOLUME_UNITY,
  isBoosted,
  positionOfVolume,
  volumeAfterWheel,
  volumeAtPosition,
  volumeLabel,
  volumeLevel,
} from "../logic/volume.ts";
import { behindGap, liveState } from "../logic/format.ts";

const near = (a, b, eps = 1e-9) => assert.ok(Math.abs(a - b) < eps, `${a} !≈ ${b}`);

// ------------------------------------------------------------------ ceiling

test("the ceiling is mpv's volume-max, not 100", () => {
  // src-tauri/src/surface/mpv.rs sets volume-max to 200 and set_volume clamps
  // to the same number. A frontend that stopped at 100 would hide half the
  // range the backend accepts.
  assert.equal(VOLUME_MAX, 200);
  assert.equal(VOLUME_UNITY, 100);
});

// ------------------------------------------------------------------- scale

test("the ends of the track are silence and full boost", () => {
  near(positionOfVolume(0), 0);
  near(positionOfVolume(VOLUME_MAX), 1);
  assert.equal(volumeAtPosition(0), 0);
  assert.equal(volumeAtPosition(1), VOLUME_MAX);
});

test("100% sits at the split, not at the middle of the track", () => {
  near(positionOfVolume(VOLUME_UNITY), UNITY_FRACTION);
  assert.equal(volumeAtPosition(UNITY_FRACTION), VOLUME_UNITY);
  // The whole point: unity is well past halfway, so the resting position of a
  // normal stream is not a hair from software gain.
  assert.ok(positionOfVolume(VOLUME_UNITY) > 0.6);
});

test("below unity the scale is linear, so half volume is half the sane range", () => {
  near(positionOfVolume(50), UNITY_FRACTION / 2);
  assert.equal(volumeAtPosition(UNITY_FRACTION / 2), 50);
  near(positionOfVolume(25), UNITY_FRACTION / 4);
});

test("the boost region is compressed: a decibel up there costs more travel", () => {
  const perUnitBelow = positionOfVolume(50) / 50;
  const perUnitAbove = (1 - positionOfVolume(VOLUME_UNITY)) / (VOLUME_MAX - VOLUME_UNITY);
  assert.ok(
    perUnitAbove < perUnitBelow,
    `boost should be tighter than normal: ${perUnitAbove} vs ${perUnitBelow}`,
  );
  // 100 units of boost share the last 30% of the track.
  near(positionOfVolume(150), UNITY_FRACTION + (1 - UNITY_FRACTION) / 2);
  assert.equal(volumeAtPosition(UNITY_FRACTION + (1 - UNITY_FRACTION) / 2), 150);
});

test("position and volume round-trip at every whole percent", () => {
  // Except inside the unity snap, where pointing near 100 deliberately
  // means 100 — see "clicking near unity means unity" below.
  for (let v = 0; v <= VOLUME_MAX; v++) {
    if (Math.abs(v - 100) <= UNITY_SNAP) continue;
    assert.equal(volumeAtPosition(positionOfVolume(v)), v, `round trip failed at ${v}`);
  }
});

test("position is monotonic across the seam", () => {
  let last = -1;
  for (let v = 0; v <= VOLUME_MAX; v++) {
    const p = positionOfVolume(v);
    assert.ok(p > last, `not increasing at ${v}: ${p} after ${last}`);
    last = p;
  }
});

test("a point on the track never asks for a volume the backend would clamp", () => {
  for (let i = -5; i <= 105; i++) {
    const v = volumeAtPosition(i / 100);
    assert.ok(v >= 0 && v <= VOLUME_MAX, `${i}% of the track gave ${v}`);
    assert.ok(Number.isInteger(v), `${v} is not a whole percent`);
  }
});

test("a degenerate ceiling falls back to a linear track rather than dividing by zero", () => {
  near(positionOfVolume(50, 100), 0.5);
  assert.equal(volumeAtPosition(0.5, 100), 50);
  assert.ok(Number.isFinite(positionOfVolume(50, 0)));
});

// ------------------------------------------------------------------- wheel

test("the wheel moves one step per notch, whatever deltaY claims", () => {
  assert.equal(volumeAfterWheel(100, -1), 100 + VOLUME_STEP);
  assert.equal(volumeAfterWheel(100, -480), 100 + VOLUME_STEP);
  assert.equal(volumeAfterWheel(100, 3), 100 - VOLUME_STEP);
});

test("the wheel can reach the boost region and stops at the ceiling", () => {
  let v = 100;
  for (let i = 0; i < 100; i++) v = volumeAfterWheel(v, -1);
  assert.equal(v, VOLUME_MAX);
  let down = 10;
  for (let i = 0; i < 100; i++) down = volumeAfterWheel(down, 1);
  assert.equal(down, 0);
});

test("a zero delta is not a volume change", () => {
  assert.equal(volumeAfterWheel(73, 0), 73);
});

test("the wheel and the drag agree: one gesture cannot reach a value the other cannot", () => {
  // The wheel works over the video and over the slider itself, and the handle
  // can be dragged. Whatever a wheel notch lands on, pointing at that spot on
  // the track has to give back the same number — otherwise the handle settles
  // a percent away from where scrolling put it and the two gestures fight.
  for (let v = 0; v <= VOLUME_MAX; v++) {
    for (const delta of [-1, 1]) {
      const wheeled = volumeAfterWheel(v, delta);
      // Inside the snap the two are meant to differ: the wheel steps by a
      // known amount from where it is, and the pointer rounds to unity.
      // They do not fight over it — the handle stays where the wheel put
      // it, and only a later click near 100 moves it there.
      if (Math.abs(wheeled - 100) <= UNITY_SNAP && wheeled !== 100) continue;
      const dragged = volumeAtPosition(positionOfVolume(wheeled));
      assert.equal(dragged, wheeled, `wheel from ${v} landed on ${wheeled}, drag said ${dragged}`);
    }
  }
});

// ------------------------------------------------------------------ labels

test("boost is a state the control can report, not a surprise", () => {
  assert.equal(isBoosted(100), false);
  assert.equal(isBoosted(101), true);
  assert.equal(isBoosted(0), false);
});

test("muted says muted rather than a number nobody is hearing", () => {
  assert.equal(volumeLabel(120, true), "muted");
  assert.equal(volumeLabel(120, false), "120%");
  assert.equal(volumeLevel(120, true), "muted");
  assert.equal(volumeLevel(0, false), "muted");
  assert.equal(volumeLevel(20, false), "low");
  assert.equal(volumeLevel(80, false), "high");
});

// --------------------------------------------------------------- live edge

test("liveness comes from the backend's flag, never from a threshold here", () => {
  // The dot follows atLiveEdge and nothing else. Both of these are states the
  // hysteresis produces: still flagged live a few seconds back, and not yet
  // flagged live while catching up.
  assert.equal(liveState(true, 4).live, true);
  assert.equal(liveState(false, 4).live, false);
  assert.equal(liveState(false, 0).live, false);
});

test("a sawtoothing behindLive does not move the indicator", () => {
  // The bug this replaced: behindLive is measured against the end of mpv's
  // seekable window, which advances in chunks, so it oscillates by a couple of
  // seconds while playback is live the whole time. Nothing rendered may depend
  // on it while the flag holds.
  const wobble = [0, 0.4, 1.2, 3.1, 2.7, 0.9, 3.9, 0.2, 2.2, 4, 1.5];
  const rendered = wobble.map((b) => JSON.stringify(liveState(true, b)));
  assert.equal(new Set(rendered).size, 1, `indicator changed across ${rendered.join(" ")}`);
  assert.deepEqual(JSON.parse(rendered[0]), { live: true, label: "Live", canJump: false });

  // And the same while it is behind: the label counts, but the state does not
  // flip back and forth.
  const behind = wobble.map((b) => liveState(false, 12 + b).live);
  assert.equal(new Set(behind).size, 1);
});

test("at the edge there is nowhere to jump, and the control says so", () => {
  assert.equal(liveState(true, 0).canJump, false);
  assert.equal(liveState(false, 12).canJump, true);
});

test("the control says its state out loud, so colour is not the only signal", () => {
  assert.equal(liveState(true, 0.2).label, "Live");
  assert.equal(liveState(false, 12).label, "Behind live by 12s");
  assert.equal(liveState(false, 95).label, "Behind live by 1m35s");
  assert.equal(behindGap(-3), "0s");
});

test("the unity snap is a target you can hit without aiming", () => {
  // 0-100 spans 70% of the track, so a unit is about 1.4px on the player
  // bar's slider and 0.8px on the docked picture's shorter one. The window
  // has to be worth several of those on the *short* one, or the snap only
  // helps where it was least needed.
  const shortTrackPx = 112 * 0.7;
  assert.ok((UNITY_SNAP / 100) * shortTrackPx >= 5, "the snap window is too fine to aim at");
  // And not so wide that it swallows a meaningful part of the range.
  assert.ok(UNITY_SNAP <= 12, "the snap has eaten the useful range around unity");
});

test("clicking near unity means unity", () => {
  // 100% is the value people come back to, and on a continuous track every
  // position is as good as its neighbour — so without a snap, "normal" is a
  // pixel-hunt that lands on 99 or 101.
  const at = (v) => volumeAtPosition(positionOfVolume(v));
  assert.equal(at(100), 100);
  assert.equal(at(100 - UNITY_SNAP), 100);
  assert.equal(at(100 + UNITY_SNAP), 100);
  // Just outside the window is left alone: nobody who aimed at 94 wants 100.
  assert.equal(at(100 - UNITY_SNAP - 2), 100 - UNITY_SNAP - 2);
  assert.equal(at(100 + UNITY_SNAP + 2), 100 + UNITY_SNAP + 2);
});

test("the ends of the track are still reachable through the snap", () => {
  assert.equal(volumeAtPosition(0), 0);
  assert.equal(volumeAtPosition(1), VOLUME_MAX);
});

test("only the pointer snaps, not the wheel", () => {
  // A step that silently became a different step would be worse than a
  // pixel-hunt. From 100 a notch has to actually move by the step, even
  // though it lands inside the snap window a click would round away.
  assert.equal(volumeAfterWheel(100, 1), 100 - VOLUME_STEP);
  assert.equal(volumeAfterWheel(100 - VOLUME_STEP, -1), 100);
  // And a notch from somewhere inside the window is still a plain step.
  assert.equal(volumeAfterWheel(104, 1), 104 - VOLUME_STEP);
});

test("the wheel steps in fives, so the readout stays round", () => {
  // Whole multiples of five from unity: 100, 95, 90 — not 96, 92, 88.
  assert.equal(VOLUME_STEP, 5);
  let v = 100;
  for (const expected of [95, 90, 85]) {
    v = volumeAfterWheel(v, 1);
    assert.equal(v, expected);
  }
});
