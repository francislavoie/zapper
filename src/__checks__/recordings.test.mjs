import test from "node:test";
import assert from "node:assert/strict";
import {
  backupCandidateLabel,
  backupMenuLabel,
  backupTargets,
  backupText,
  bytes,
  canRecord,
  conflictLosesProgramme,
  conflictText,
  episodeLabel,
  groupRecordings,
  markKey,
  marks,
  padMinutes,
  padSeconds,
  POLL_BUSY_MS,
  POLL_IDLE_MS,
  POLL_OPEN_MS,
  pollDelay,
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

test("a recording being saved keeps its mark, and is listed as saving", () => {
  const recs = [rec({ id: 1, state: "finishing", parts: [{ path: "/v/x.ts", streamId: 7, bytes: 2e9, started: 0, ended: 1 }] })];
  assert.deepEqual([...marks(recs).keys()], [markKey(7, NOON)]);
  assert.equal(recordingStatus(recs[0], NOON), "Saving 2.0 GB…");
  assert.deepEqual(
    groupRecordings([...recs, rec({ id: 2, state: "recording" }), rec({ id: 3, state: "done" })]).map((g) => g.label),
    ["Recording", "Saving", "Recorded"],
  );
});

test("the list is followed closely while anything records or saves, and just after an action", () => {
  const quiet = [rec({ state: "scheduled" }), rec({ state: "done" })];
  assert.equal(pollDelay(quiet, false, 60_000), POLL_IDLE_MS);
  assert.equal(pollDelay(quiet, true, 60_000), POLL_OPEN_MS);
  assert.equal(pollDelay(null, false, 60_000), POLL_IDLE_MS, "no service: the slow pace");
  for (const state of ["recording", "finishing"]) {
    assert.equal(pollDelay([...quiet, rec({ state })], false, 60_000), POLL_BUSY_MS, state);
  }
  // For ten seconds after something was asked of the service.
  assert.equal(pollDelay(quiet, false, 0), POLL_BUSY_MS);
  assert.equal(pollDelay(quiet, false, 9_999), POLL_BUSY_MS);
  assert.equal(pollDelay(quiet, false, 10_000), POLL_IDLE_MS);
  assert.equal(pollDelay(null, true, 3_000), POLL_BUSY_MS);
  assert.ok(POLL_BUSY_MS <= 2_000 && POLL_IDLE_MS >= 30_000);
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

test("the guide's season and episode go along when it has them", () => {
  const ch = { streamId: 9, name: "ONE HD", cleanName: "One HD" };
  const p = { start: 10, stop: 20, title: "Harbour Lights", description: null };
  assert.deepEqual(scheduleRequest(ch, { ...p, season: 2, episode: 4 }), {
    streamId: 9,
    channelName: "One HD",
    title: "Harbour Lights",
    start: 10,
    stop: 20,
    season: 2,
    episode: 4,
  });
  const e = scheduleRequest(ch, { ...p, episode: 12 });
  assert.equal(e.episode, 12);
  assert.ok(!("season" in e));
  // A season alone names nothing.
  assert.ok(!("season" in scheduleRequest(ch, { ...p, season: 3 })));
});

test("a programme is offered as backup for overlapping recordings on other channels", () => {
  const recs = [
    rec({ id: 1, streamId: 7, start: NOON, stop: NOON + 1800 }),
    rec({ id: 2, streamId: 8, start: NOON + 900, stop: NOON + 2700, state: "recording" }),
    rec({ id: 3, streamId: 9, start: NOON + 1800, stop: NOON + 3600 }),
    rec({ id: 4, streamId: 10, start: NOON - 600, stop: NOON + 600, state: "done" }),
    rec({ id: 5, streamId: 11, start: NOON + 60, stop: NOON + 120 }),
    rec({ id: 6, streamId: 12, start: NOON + 100, stop: NOON + 200 }),
  ];
  const p = { start: NOON, stop: NOON + 1800 };
  // Not its own channel (7), not a finished one (4), not one that starts as
  // it ends (3); soonest first, three at most.
  assert.deepEqual(
    backupTargets(recs, 7, p).map((r) => r.id),
    [5, 6, 2],
  );
  assert.deepEqual(
    backupTargets(recs, 99, p, 10).map((r) => r.id),
    [1, 5, 6, 2],
  );
  assert.deepEqual(backupTargets(recs, 7, { start: NOON + 4000, stop: NOON + 5000 }), []);
});

test("the menu offers to use a channel as backup, or to stop", () => {
  assert.equal(backupMenuLabel({ title: "Film", backupStreamId: null }, 5), "Use as backup for “Film”");
  assert.equal(backupMenuLabel({ title: "Film", backupStreamId: 6 }, 5), "Use as backup for “Film”");
  assert.equal(backupMenuLabel({ title: "Film", backupStreamId: 5 }, 5), "Stop using as backup for “Film”");
});

test("a recording's backup and the candidates read plainly", () => {
  assert.equal(backupText({ backupStreamId: null }), "No backup channel");
  assert.equal(backupText({ backupStreamId: 4, backupName: "One SD" }), "Backup: One SD");
  assert.equal(backupText({ backupStreamId: 4 }), "Backup: channel 4");
  assert.equal(backupCandidateLabel({ streamId: 1, name: "One HD", reason: "guide", height: 1080 }), "One HD · 1080p");
  assert.equal(
    backupCandidateLabel({ streamId: 2, name: "Sim", reason: "programme", failing: true }),
    "Sim · failed last time · same programme",
  );
});

test("episode labels read as recordings are named", () => {
  assert.equal(episodeLabel({ season: 2, episode: 4 }), "S02E04");
  assert.equal(episodeLabel({ season: null, episode: 12 }), "E12");
  assert.equal(episodeLabel({ season: 3 }), "");
  assert.equal(episodeLabel({}), "");
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

test("a conflict says what is lost, and to which recording", () => {
  const c = (over) => ({ title: "Film", with: "Match", kind: "cutEnd", secs: 240, streams: 1, ...over });
  assert.equal(conflictText(c()), "Overlaps “Match” on another channel: the last 4m will be cut");
  assert.equal(conflictText(c({ kind: "lateStart", secs: 60 })), "Overlaps “Match” on another channel: starts 1m late");
  assert.equal(
    conflictText(c({ kind: "lostStart", secs: 1800 })),
    "Overlaps “Match” on another channel and the account allows 1 stream: the first 30m will be missed",
  );
  assert.equal(
    conflictText(c({ kind: "missed", secs: 0, streams: 2 })),
    "Overlaps “Match” on another channel and the account allows 2 streams: it can’t be recorded",
  );
  // In a preview, about the recording already scheduled that would lose.
  assert.equal(conflictText(c({ other: true })), "Overlaps “Film” on another channel: its last 4m will be cut");
  assert.ok(conflictLosesProgramme(c({ kind: "missed" })));
  assert.ok(conflictLosesProgramme(c({ kind: "lostStart" })));
  assert.ok(!conflictLosesProgramme(c({ kind: "cutEnd" })));
  assert.ok(!conflictLosesProgramme(c({ kind: "lateStart" })));
});
