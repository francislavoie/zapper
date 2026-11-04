import test from "node:test";
import assert from "node:assert/strict";
import {
  activeRecordingLine,
  recordingIndicator,
  backupCandidateLabel,
  backupMenuLabel,
  backupTargets,
  backupText,
  bytes,
  canRecord,
  keepSelected,
  recordingsRailCount,
  conflictLosesProgramme,
  conflictText,
  episodeLabel,
  markKey,
  marks,
  padMinutes,
  padSeconds,
  playerRecord,
  programmeOnNow,
  POLL_BUSY_MS,
  POLL_IDLE_MS,
  POLL_OPEN_MS,
  pollDelay,
  recordingSections,
  recordingRowStatus,
  visibleSelection,
  recordingFacts,
  recordingStatus,
  recordingWhen,
  RECORDING_DEFAULTS,
  scheduleRequest,
  strmReach,
  WHILE_RECORDING,
} from "../logic/recordings.ts";
import { setTimeFormat } from "../logic/format.ts";

test("what other programs see while recording: three choices, hidden by default", () => {
  assert.deepEqual(
    WHILE_RECORDING.map((w) => w.id),
    ["hidden", "visible", "strm"],
  );
  assert.equal(RECORDING_DEFAULTS.whileRecording, "hidden");
  assert.ok(WHILE_RECORDING.every((w) => w.label && w.tip));
});

test("the .strm address says who can reach it", () => {
  // Network access off: this computer only, unless a host was typed.
  assert.equal(strmReach("http://127.0.0.1:8796", false), "local");
  assert.equal(strmReach("http://127.0.0.1:8796", true), "fine");
  assert.equal(strmReach("http://[::1]:8796", false), "local");
  assert.equal(strmReach("https://192.168.1.20:8796", false), "fine");
  assert.equal(strmReach("http://192.168.1.20:8796", false), "plain");
  assert.equal(strmReach("http://nas.local:8796", true), "plain");
  assert.equal(strmReach("", false), "fine");
});

const NOON = Math.floor(new Date(2026, 8, 8, 12, 0, 0).getTime() / 1000);

const part = (b) => ({ path: "x", streamId: 7, bytes: b, started: 0, ended: null });

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
    recordingSections([...recs, rec({ id: 2, state: "recording" }), rec({ id: 3, state: "done" })], "all").map((s) => s.label),
    ["Recording now", "Recorded"],
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

test("the title bar's indicator counts what records and lists each", () => {
  setTimeFormat("24h");
  const now = NOON + 23 * 60 + 30;
  const one = rec({
    id: 1,
    state: "recording",
    channelName: "Northwind One",
    title: "Harbour Lights",
    start: NOON,
    stop: NOON + 3600,
    padBefore: 60,
    padAfter: 300,
    parts: [{ path: "/v/a.ts", streamId: 7, bytes: 1, started: NOON - 60, ended: null }],
  });
  assert.equal(recordingIndicator(null, now), null, "no service: nothing");
  assert.equal(recordingIndicator([rec({ state: "scheduled" }), rec({ state: "done" })], now), null);
  const i = recordingIndicator([one], now);
  assert.equal(i.label, "Recording");
  assert.equal(i.tone, "live");
  assert.equal(i.tip, "Northwind One · Harbour Lights — 24m, until 13:05");

  // A second one waiting for a connection, on its backup, with a conflict.
  const two = rec({
    id: 2,
    state: "recording",
    streamId: 8,
    backupStreamId: 9,
    backupName: "Meridian SD",
    channelName: "Meridian",
    title: "Quayside",
    start: NOON + 600,
    stop: NOON + 7200,
    padBefore: 0,
    padAfter: 0,
    waiting: "waiting for a free connection (1 of 1 in use)",
    parts: [
      { path: "/v/b1.ts", streamId: 8, bytes: 1, started: NOON + 600, ended: NOON + 700 },
      { path: "/v/b2.ts", streamId: 9, bytes: 1, started: NOON + 700, ended: null },
    ],
    conflicts: [{ title: "Quayside", with: "Harbour Lights", kind: "lateStart", secs: 60, streams: 1 }],
  });
  const saving = rec({ id: 3, state: "finishing", channelName: "Cove", title: "Old Film" });
  const many = recordingIndicator([saving, two, one], now);
  assert.equal(many.label, "Recording 2");
  assert.deepEqual(many.tip.split("\n"), [
    "Northwind One · Harbour Lights — 24m, until 13:05",
    "Meridian · Quayside — 13m, until 14:00",
    "↳ Waiting for a free connection (1 of 1 in use)",
    "↳ On its backup channel, Meridian SD",
    "↳ Overlaps “Harbour Lights” on another channel: starts 1m late",
    "Cove · Old Film — saving into its file…",
  ]);
  const left = recordingIndicator([saving], now);
  assert.deepEqual([left.label, left.tone], ["Saving", "saving"]);
  // Under a minute in, the seconds.
  assert.match(activeRecordingLine({ ...one, parts: [] , start: now - 20, padBefore: 0 }, now), /— 20s, until/);
});

test("anything not over can be recorded", () => {
  assert.ok(canRecord({ stop: NOON + 1 }, NOON));
  assert.ok(!canRecord({ stop: NOON }, NOON));
});

test("the player records the programme on now, while it plays live", () => {
  const news = { title: "News", start: NOON - 600, stop: NOON + 1200 };
  assert.equal(programmeOnNow(news, null, NOON), news);
  // Up to its very end, unlike the sleep timer's.
  assert.equal(programmeOnNow(news, null, NOON + 1199), news);
  assert.equal(programmeOnNow(news, null, NOON + 1200), null);
  assert.equal(programmeOnNow({ ...news, start: NOON + 60 }, null, NOON), null);
  assert.equal(programmeOnNow(null, null, NOON), null);
  // A catch-up programme already aired: what the guide says is on now is
  // not what plays.
  const aired = { title: "Earlier", start: NOON - 7200, stop: NOON - 3600 };
  assert.equal(programmeOnNow(news, aired, NOON), null);
  // The channel opened for the programme on now is live.
  assert.equal(programmeOnNow(news, news, NOON), news);
});

test("Record from the player offers the programme, or its recording", () => {
  const ch = { streamId: 7, name: "ONE", cleanName: "One" };
  const p = { title: "News", start: NOON, stop: NOON + 1800 };
  assert.deepEqual(playerRecord([], ch, p, NOON + 60), { kind: "record", channel: ch, programme: p });
  // Nothing to offer without the service, a programme, or a channel.
  assert.equal(playerRecord(null, ch, p, NOON + 60), null);
  assert.equal(playerRecord([], ch, null, NOON + 60), null);
  assert.equal(playerRecord([], null, p, NOON + 60), null);
  assert.equal(playerRecord([], { ...ch, isSeparator: true }, p, NOON + 60), null);
  assert.equal(playerRecord([], ch, p, NOON + 1800), null);
  // Already scheduled, recording or being saved: that recording.
  for (const [state, kind] of [
    ["scheduled", "scheduled"],
    ["recording", "recording"],
    ["finishing", "saving"],
  ]) {
    const r = rec({ state });
    const got = playerRecord([r], ch, p, NOON + 60);
    assert.equal(got.kind, kind, state);
    assert.equal(got.rec, r);
  }
  // A finished recording of it, or one of another programme, is no reason
  // not to record.
  assert.equal(playerRecord([rec({ state: "done" })], ch, p, NOON + 60).kind, "record");
  assert.equal(playerRecord([rec({ state: "recording", start: NOON - 3600 })], ch, p, NOON + 60).kind, "record");
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

test("one list: recording now, then scheduled soonest first, then recorded newest first", () => {
  const all = [rec({ id: 1, state: "done", start: NOON - 7200 }), rec({ id: 2, state: "scheduled", start: NOON + 7200 }),
    rec({ id: 3, state: "failed", start: NOON - 3600 }), rec({ id: 4, state: "scheduled", start: NOON + 3600 }),
    rec({ id: 5, state: "finishing" }), rec({ id: 6, state: "recording", start: NOON - 60 })];
  const s = recordingSections(all, "all");
  assert.deepEqual(s.map((x) => x.label), ["Recording now", "Scheduled", "Recorded"]);
  assert.deepEqual(s.map((x) => x.items.map((r) => r.id)), [[6, 5], [4, 2], [3, 1]]);
  assert.deepEqual(recordingSections(all, "scheduled").map((x) => x.id), ["now", "scheduled"]);
  assert.deepEqual(recordingSections(all, "recorded").map((x) => x.id), ["recorded"]);
  assert.deepEqual(recordingSections([], "all"), []);
});
test("a row's status: conflicts first, then how soon, then its backup", () => {
  const conflict = { kind: "cutEnd", secs: 240, with: "Match", streams: 1 };
  assert.equal(recordingRowStatus(rec({ conflicts: [conflict] }), NOON - 86400).tone, "warn");
  assert.equal(recordingRowStatus(rec({ start: NOON + 3660 + 60 }), NOON).text, "starts in 1h01m"); // padded start in 3660 s
  assert.equal(recordingRowStatus(rec({ start: NOON + 13 * 3600, backupStreamId: 9, backupName: "One HD" }), NOON).text, "backup: One HD");
  assert.equal(recordingRowStatus(rec({ start: NOON + 13 * 3600 }), NOON).text, "");
});
test("a row's status while recording, once recorded, and when it failed", () => {
  const r = recordingRowStatus(rec({ state: "recording", start: NOON - 600, parts: [part(5e8)] }), NOON);
  assert.match(r.text, /^recording · .+ left$/);
  assert.ok(r.progress > 0 && r.progress < 1);
  assert.deepEqual(recordingRowStatus(rec({ state: "done", parts: [part(1.4e9)] }), NOON), { text: "✓ 1 file · 1.4 GB", tone: "good", progress: null });
  assert.equal(recordingRowStatus(rec({ state: "done", parts: [part(3e9), part(2.8e9)] }), NOON).text, "✓ 2 parts · 5.8 GB");
  assert.deepEqual(recordingRowStatus(rec({ state: "failed", error: "no feed delivered" }), NOON), { text: "No feed delivered", tone: "bad", progress: null });
});
test("a filter that hides the selected recording unselects it", () => {
  const s = recordingSections([rec({ id: 1, state: "done" }), rec({ id: 2 })], "recorded");
  assert.equal(visibleSelection(s, 1), 1);
  assert.equal(visibleSelection(s, 2), null);
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

test("the rail says how many record now and how many are coming", () => {
  const s = (state) => ({ state });
  assert.equal(recordingsRailCount([s("recording"), s("scheduled"), s("scheduled"), s("done")]), "1 now · 2 next");
  assert.equal(recordingsRailCount([s("scheduled")]), "1 next");
  assert.equal(recordingsRailCount([s("recording"), s("finishing")]), "1 now");
  assert.equal(recordingsRailCount([s("done"), s("failed")]), "");
  assert.equal(recordingsRailCount(null), "");
});
test("a selected recording that is gone is no longer selected", () => {
  assert.equal(keepSelected([rec({ id: 3 })], 3), 3);
  assert.equal(keepSelected([rec({ id: 3 })], 4), null);
  assert.equal(keepSelected(null, 3), 3, "a service that does not answer leaves the choice");
  assert.equal(keepSelected(null, null), null);
  assert.equal(keepSelected([rec({ id: 3 })], null), null);
});

test("the band's line about padding and where the files go", () => {
  assert.equal(recordingFacts(rec({})), "Padding 1 min before, 5 after · Folder /v/News");
  assert.equal(recordingFacts(rec({ padBefore: 0, padAfter: 0 })), "No padding · Folder /v/News");
  assert.equal(recordingFacts(rec({ padBefore: 0, padAfter: 120 })), "Padding 0 min before, 2 after · Folder /v/News");
});
