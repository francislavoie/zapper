import test from "node:test";
import assert from "node:assert/strict";
import {
  LIVE_MARGIN,
  UNSUPPORTED,
  aspectFit,
  atLiveEdge,
  browserCan,
  bufferPosition,
  canPlayHere,
  endingOf,
  liveTarget,
  meansAnyway,
  noticeText,
  refusalState,
  seekTarget,
} from "../logic/webPlayback.ts";
import { endedActions, endedMessage } from "../logic/ending.ts";

const host = (types, managed = false) => {
  const ms = { isTypeSupported: (t) => types.some((x) => t.includes(x)) };
  return managed ? { ManagedMediaSource: ms } : { MediaSource: ms };
};

test("a browser plays where it has Media Source Extensions that take H.264 and AAC", () => {
  assert.equal(canPlayHere(host(["avc1"])), true);
  assert.equal(canPlayHere(host(["avc1"], true)), true, "iOS 17.1's managed variant");
  assert.equal(canPlayHere({}), false, "an iPhone before iOS 17.1");
  assert.equal(canPlayHere(host([])), false);
  assert.equal(
    canPlayHere({ MediaSource: { isTypeSupported: () => { throw new Error("no"); } } }),
    false,
  );
  assert.match(UNSUPPORTED, /iOS 17\.1/);
});

test("the service is told what the browser decodes beyond H.264 and AAC", () => {
  assert.deepEqual(browserCan(host(["avc1"])), { hevc: false, ac3: false, eac3: false });
  assert.deepEqual(browserCan(host(["hvc1", "ac-3", "ec-3"])), { hevc: true, ac3: true, eac3: true });
  assert.deepEqual(browserCan({}), { hevc: false, ac3: false, eac3: false });
});

test("the picture's place in the buffer", () => {
  assert.deepEqual(bufferPosition([], 0), { bufferedBack: 0, behindLive: 0 });
  const r = [[10, 100]];
  assert.deepEqual(bufferPosition(r, 100 - LIVE_MARGIN), { bufferedBack: 90 - LIVE_MARGIN, behindLive: 0 });
  assert.deepEqual(bufferPosition(r, 40), { bufferedBack: 30, behindLive: 100 - LIVE_MARGIN - 40 });
  // A gap: what is behind counts from the range the picture is in.
  assert.equal(bufferPosition([[0, 20], [30, 60]], 40).bufferedBack, 10);
});

test("live has the app's hysteresis", () => {
  assert.equal(atLiveEdge(true, 7), true);
  assert.equal(atLiveEdge(true, 9), false);
  assert.equal(atLiveEdge(false, 5), false);
  assert.equal(atLiveEdge(false, 2), true);
});

test("skips land inside what is held and never past live", () => {
  const r = [[10, 100]];
  assert.equal(seekTarget([], 5, -10), null);
  assert.equal(seekTarget(r, 50, -10), 40);
  assert.equal(seekTarget(r, 15, -10), 10.2, "not before the oldest frame");
  assert.equal(seekTarget(r, 95, 10), 100 - LIVE_MARGIN, "not past live");
  assert.equal(liveTarget(r), 100 - LIVE_MARGIN);
  assert.equal(liveTarget([]), null);
});

test("another device on the account is said as the app says it, with Watch here anyway", () => {
  const r = refusalState(409, {
    error: "Your account is in use on another device (1 of 1 connection).",
    reason: "inUse",
    anyway: true,
    slots: { active: 1, max: 1 },
  });
  assert.equal(r.ending.kind, "inUse");
  const p = { state: "failed", ending: r.ending, error: r.error, reconnect: null };
  assert.match(endedMessage(p).title, /in use on another device \(1 of 1 connection\)/);
  assert.equal(endedActions(p).primary, "watchHere");
  assert.equal(meansAnyway(r.ending), true, "pressing it again means anyway");

  // Another browser on the service is the same thing.
  assert.equal(refusalState(409, { reason: "viewers", error: "x" }).ending.kind, "inUse");
});

test("a recording in the way is never 'anyway': its words, and Retry", () => {
  const r = refusalState(409, {
    error: "Every connection the account allows is in use, recording “The Film” on One.",
    reason: "recording",
    anyway: false,
    watchInstead: [7],
  });
  assert.equal(r.ending, null);
  const p = { state: "failed", ending: null, error: r.error, reconnect: null };
  assert.equal(endedMessage(p).title, "The account's connection is recording.");
  assert.match(endedMessage(p).detail, /recording “The Film” on One/);
  assert.equal(endedActions(p).primary, "retry");
  assert.equal(meansAnyway(r.ending), false);
});

test("a channel the browser can't play says so", () => {
  const r = refusalState(422, {
    error: "Its video is MPEG-2, which browsers can’t play.",
    reason: "notPlayable",
  });
  assert.equal(r.ending, null);
  const m = endedMessage({ state: "failed", ending: null, error: r.error });
  assert.equal(m.title, "Not playable in the browser.");
  assert.match(m.detail, /^Its video is MPEG-2/);
  // An error of one line is the app's: its own headline stays.
  assert.equal(
    endedMessage({ state: "failed", ending: null, error: "boom" }).title,
    "Couldn't play this channel.",
  );
});

test("a feed that failed is told as the app tells it", () => {
  const r = refusalState(502, {
    error: "The channel didn't start.",
    reason: "feed",
    ending: { kind: "refused", status: 509, limit: true },
  });
  assert.deepEqual(r.ending, { kind: "refused", slots: null, status: 509, limit: true });
  assert.equal(endingOf({ kind: "nonsense", status: null, limit: false }), null);
  assert.equal(refusalState(0, null).ending.kind, "network");
});

test("the warning names the recording, its channel and its time", () => {
  const n = {
    recordingId: 3,
    title: "The Match",
    channel: "Sport One",
    streamId: 9,
    from: 1000,
    start: 1060,
    waiting: false,
  };
  assert.equal(
    noticeText(n, () => "21:00"),
    "“The Match” starts recording on Sport One at 21:00 and needs this connection.",
  );
  assert.match(noticeText({ ...n, waiting: true }, () => ""), /can’t start recording on Sport One while you watch/);
});

test("a forced aspect fits the box it is in", () => {
  const box = { width: 1600, height: 900 };
  assert.deepEqual(aspectFit("auto", box, 16 / 9), { fit: "contain", sx: 1, sy: 1 });
  assert.equal(aspectFit("fill", box, 4 / 3).fit, "cover");
  assert.equal(aspectFit("stretch", box, 4 / 3).fit, "fill");
  // 4:3 shown as 16:9 in a 16:9 box: wider by 4/3, as tall.
  const f = aspectFit("16:9", box, 4 / 3);
  assert.equal(f.fit, "contain");
  assert.ok(Math.abs(f.sx - 4 / 3) < 1e-9 && Math.abs(f.sy - 1) < 1e-9, JSON.stringify(f));
  // Nothing known of the picture yet: left alone.
  assert.deepEqual(aspectFit("4:3", box, 0), { fit: "contain", sx: 1, sy: 1 });
});
