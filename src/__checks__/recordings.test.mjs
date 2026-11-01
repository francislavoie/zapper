import test from "node:test";
import assert from "node:assert/strict";
import {
  bytes,
  canRecord,
  groupRecordings,
  markKey,
  marks,
  padMinutes,
  padSeconds,
  recordingStatus,
  recordingWhen,
  scheduleRequest,
} from "../logic/recordings.ts";
import { setTimeFormat } from "../logic/format.ts";

const NOON = Math.floor(new Date(2026, 8, 8, 12, 0, 0).getTime() / 1000);

const rec = (over) => ({
  id: 1,
  streamId: 7,
  backupStreamId: null,
  channelName: "One",
  title: "News",
  start: NOON,
  stop: NOON + 1800,
  padBefore: 60,
  padAfter: 300,
  state: "scheduled",
  parts: [],
  notes: [],
  folder: "/v/News",
  ...over,
});

test("only scheduled and recording programmes are marked in the guide", () => {
  const m = marks([
    rec({ id: 1, state: "scheduled" }),
    rec({ id: 2, state: "recording", start: NOON + 3600 }),
    rec({ id: 3, state: "done", start: NOON + 7200 }),
    rec({ id: 4, state: "failed", start: NOON + 9000 }),
  ]);
  assert.deepEqual([...m.keys()], [markKey(7, NOON), markKey(7, NOON + 3600)]);
});

test("anything not over can be recorded", () => {
  assert.ok(canRecord({ stop: NOON + 1 }, NOON));
  assert.ok(!canRecord({ stop: NOON }, NOON));
});

test("a schedule request names the channel and the programme", () => {
  const ch = { streamId: 9, name: "ONE HD", cleanName: "One HD" };
  assert.deepEqual(scheduleRequest(ch, { start: 10, stop: 20, title: "News" }), {
    streamId: 9,
    channelName: "One HD",
    title: "News",
    start: 10,
    stop: 20,
  });
});

test("the list groups by state, soonest coming up and newest recorded first", () => {
  const g = groupRecordings([
    rec({ id: 1, state: "done", start: NOON - 7200 }),
    rec({ id: 2, state: "scheduled", start: NOON + 7200 }),
    rec({ id: 3, state: "scheduled", start: NOON + 3600 }),
    rec({ id: 4, state: "done", start: NOON - 3600 }),
    rec({ id: 5, state: "recording" }),
  ]);
  assert.deepEqual(
    g.map((x) => [x.label, x.items.map((r) => r.id)]),
    [
      ["Recording", [5]],
      ["Scheduled", [3, 2]],
      ["Recorded", [4, 1]],
    ],
  );
});

test("rows say when and how it is going", () => {
  // Times of day follow the clock setting, like every other one shown.
  setTimeFormat("24h");
  assert.equal(recordingWhen(rec({}), NOON), "Today 12:00–12:30");
  setTimeFormat("12h");
  assert.equal(recordingWhen(rec({}), NOON), "Today 12:00–12:30 pm");
  setTimeFormat("24h");
  assert.equal(recordingStatus(rec({ start: NOON + 3660 }), NOON), "starts in 1h");
  assert.equal(recordingStatus(rec({ start: NOON + 30 }), NOON), "starting");
  const part = (b) => ({ path: "x", streamId: 7, bytes: b, started: 0, ended: null });
  assert.equal(recordingStatus(rec({ state: "recording", parts: [part(5e8)] }), NOON), "500 MB so far");
  assert.equal(
    recordingStatus(rec({ state: "recording", waiting: "waiting for a free connection (1 of 1 in use)" }), NOON),
    "Waiting for a free connection (1 of 1 in use)",
  );
  assert.equal(recordingStatus(rec({ state: "done", parts: [part(1e9), part(2e8)] }), NOON), "1.2 GB in 2 parts");
  assert.equal(recordingStatus(rec({ state: "failed", error: "missed: the service was off" }), NOON), "Missed: the service was off");
});

test("sizes and padding", () => {
  assert.equal(bytes(999), "999 B");
  assert.equal(bytes(1500), "1.5 kB");
  assert.equal(bytes(740e6), "740 MB");
  assert.equal(padMinutes(300), 5);
  assert.equal(padSeconds(2), 120);
  assert.equal(padSeconds(-3), 0);
});
