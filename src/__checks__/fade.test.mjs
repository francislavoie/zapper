import test from "node:test";
import assert from "node:assert/strict";
import { IDLE_MS, atScreenEdge, initialFade, nextFade } from "../logic/fade.ts";
import { volumeAfterWheel, VOLUME_MAX } from "../logic/volume.ts";
import { clock, liveOffset, measuredLine, programmeProgress, qualityBadge, looksBroken } from "../logic/format.ts";

// ------------------------------------------------------------------- fade

test("controls start visible", () => {
  const s = initialFade(1000);
  assert.equal(s.visible, true);
});

test("five idle seconds hides the controls, and not a moment sooner", () => {
  let s = initialFade(0);
  s = nextFade(s, { type: "tick" }, IDLE_MS - 1);
  assert.equal(s.visible, true);
  s = nextFade(s, { type: "tick" }, IDLE_MS);
  assert.equal(s.visible, false);
});

test("activity brings them back and restarts the clock", () => {
  let s = initialFade(0);
  s = nextFade(s, { type: "tick" }, IDLE_MS);
  assert.equal(s.visible, false);
  s = nextFade(s, { type: "activity" }, 6000);
  assert.equal(s.visible, true);
  s = nextFade(s, { type: "tick" }, 6000 + IDLE_MS - 1);
  assert.equal(s.visible, true);
  s = nextFade(s, { type: "tick" }, 6000 + IDLE_MS);
  assert.equal(s.visible, false);
});

test("leaving the window hides them immediately, not in five seconds", () => {
  let s = initialFade(0);
  s = nextFade(s, { type: "activity" }, 100);
  s = nextFade(s, { type: "leave" }, 101);
  assert.equal(s.visible, false);
});

test("a screen edge in fullscreen hides them immediately", () => {
  let s = initialFade(0);
  s = nextFade(s, { type: "edge" }, 50);
  assert.equal(s.visible, false);
});

test("pinning holds them open past the idle timeout", () => {
  let s = initialFade(0);
  s = nextFade(s, { type: "pin", pinned: true }, 10);
  s = nextFade(s, { type: "tick" }, 10 + IDLE_MS * 10);
  assert.equal(s.visible, true);
  // Unpinning restarts the clock rather than hiding on the spot.
  s = nextFade(s, { type: "pin", pinned: false }, 100_000);
  assert.equal(s.visible, true);
  s = nextFade(s, { type: "tick" }, 100_000 + IDLE_MS);
  assert.equal(s.visible, false);
});

test("leaving beats pinning — the pointer is gone", () => {
  let s = initialFade(0);
  s = nextFade(s, { type: "pin", pinned: true }, 10);
  s = nextFade(s, { type: "leave" }, 20);
  assert.equal(s.visible, false);
  assert.equal(s.pinned, false);
});

test("ticking while hidden changes nothing", () => {
  let s = initialFade(0);
  s = nextFade(s, { type: "tick" }, IDLE_MS);
  const after = nextFade(s, { type: "tick" }, IDLE_MS * 4);
  assert.equal(after, s);
});

test("edges are only the outer few pixels", () => {
  assert.ok(atScreenEdge(0, 500, 1920, 1080));
  assert.ok(atScreenEdge(960, 1079, 1920, 1080));
  assert.ok(atScreenEdge(1919, 500, 1920, 1080));
  assert.ok(!atScreenEdge(960, 540, 1920, 1080));
  assert.ok(!atScreenEdge(100, 100, 1920, 1080));
});

// ----------------------------------------------------------------- volume

test("scrolling up is louder, down is quieter", () => {
  assert.equal(volumeAfterWheel(100, -1), 104);
  assert.equal(volumeAfterWheel(100, 1), 96);
  assert.equal(volumeAfterWheel(100, 0), 100);
});

test("only the sign of deltaY matters, not the device's units", () => {
  assert.equal(volumeAfterWheel(100, -3), volumeAfterWheel(100, -240));
});

test("volume goes above 100 but stops at the ceiling", () => {
  let v = 100;
  for (let i = 0; i < 100; i++) v = volumeAfterWheel(v, -1);
  assert.equal(v, VOLUME_MAX);
  assert.ok(VOLUME_MAX > 100);
});

test("volume never goes below zero", () => {
  let v = 10;
  for (let i = 0; i < 100; i++) v = volumeAfterWheel(v, 1);
  assert.equal(v, 0);
});

// ----------------------------------------------------------------- format

test("a null quality shows no badge at all — unknown is not bad", () => {
  assert.equal(qualityBadge(null), null);
  assert.equal(looksBroken(null), false);
});

test("the badge reports measured height, whatever the channel calls itself", () => {
  const q = (height, isHdr = false) => ({
    codec: "h264",
    width: null,
    height,
    fps: null,
    isHdr,
    okCount: 1,
    failCount: 0,
  });
  // "CA: HARBOUR SPORTS 1 4K" measured at 720p is a 720p channel.
  assert.equal(qualityBadge(q(720)), "720p");
  assert.equal(qualityBadge(q(1080)), "1080p");
  assert.equal(qualityBadge(q(2160)), "4K");
  assert.equal(qualityBadge(q(2160, true)), "4K HDR");
});

test("a channel is only called broken once it has actually failed repeatedly", () => {
  const q = (okCount, failCount) => ({
    codec: null,
    width: null,
    height: null,
    fps: null,
    isHdr: false,
    okCount,
    failCount,
  });
  assert.equal(looksBroken(q(0, 1)), false); // one bad night is not a verdict
  assert.equal(looksBroken(q(0, 5)), true);
  assert.equal(looksBroken(q(10, 3)), false);
});

test("the measured line is empty until something has been measured", () => {
  const base = {
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
  };
  assert.equal(measuredLine(base), "");
  assert.equal(
    measuredLine({ ...base, width: 1280, height: 720, fps: 50, codec: "h264" }),
    "1280×720 · 50 fps · h264",
  );
});

test("clock and live offset read the way a person would say them", () => {
  assert.equal(clock(0), "0:00");
  assert.equal(clock(9), "0:09");
  assert.equal(clock(75), "1:15");
  assert.equal(clock(3661), "1:01:01");
  assert.equal(clock(-5), "0:00");
  assert.equal(liveOffset(0), "LIVE");
  assert.equal(liveOffset(0.4), "LIVE");
  assert.equal(liveOffset(90), "-1:30");
});

test("programme progress stays inside 0-1", () => {
  assert.equal(programmeProgress(100, 200, 150), 0.5);
  assert.equal(programmeProgress(100, 200, 50), 0);
  assert.equal(programmeProgress(100, 200, 900), 1);
  assert.equal(programmeProgress(100, 100, 100), 0);
});
