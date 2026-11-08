// Where a title was left: when it counts as in progress, what is said of it,
// and where an open starts. Run: node --test src/__checks__/

import assert from "node:assert/strict";
import { test } from "node:test";
import { setTimeFormat } from "../logic/format.ts";
import {
  heldAfter,
  holdStep,
  holding,
  nextStart,
  placeReportDue,
  playsOnReopen,
  progressOf,
  promptLine,
  promptStep,
  seenFor,
  resumeLabel,
  showPlayStart,
  resumePoint,
  startPlan,
  timeLeft,
  watchedLine,
  watchedWhen,
} from "../logic/resume.ts";

const place = (position, o = {}) => ({ watched: false, position, duration: 100 * 60, durationSecs: null, ...o });

test("a place counts while in progress, of a known length", () => {
  assert.equal(resumePoint(place(30 * 60)), 1800);
  assert.equal(resumePoint(place(0.5 * 60)), null);                 // under 1 %
  assert.equal(resumePoint(place(92 * 60)), null);                  // watched from 92 %
  assert.equal(resumePoint(place(1800, { watched: true })), null);
  assert.equal(resumePoint(place(1800, { duration: null })), null); // no length known
  assert.equal(resumePoint(place(1800, { duration: null, durationSecs: 6000 })), 1800);
  assert.equal(resumePoint(place(null)), null);
});

test("progress is a fraction while in progress, the player's length first", () => {
  assert.equal(progressOf(place(30 * 60)), 0.3);
  assert.equal(progressOf(place(1000, { duration: 4000, durationSecs: 2000 })), 0.25);
  assert.equal(progressOf(place(95 * 60)), null);
});

test("what the band and the question say", () => {
  assert.equal(resumeLabel(46 * 60 + 10), "Resume from 46:10");
  assert.equal(timeLeft(place(86 * 60)), "14m left");
  assert.equal(timeLeft(place(99.5 * 60, { duration: 100 * 60 })), "");   // past 92 %
  const now = Date.UTC(2026, 10, 8, 12) / 1000, yesterday = now - 86_400;
  assert.equal(watchedWhen(now - 60, now), "today");
  assert.equal(watchedWhen(yesterday, now), "yesterday");
  assert.equal(promptLine({ item: { kind: "movie", id: 2 }, title: "A Film", position: 2770, playedAt: null }, now), "You stopped at 46:10.");
});

test("a time left is at least a minute", () => {
  assert.equal(timeLeft({ watched: false, position: 5990 * 0.9, duration: 5990, durationSecs: null }), "10m left");
  assert.equal(timeLeft({ watched: false, position: 91.5, duration: 100, durationSecs: null }), "1m left");
});

test("when a title was watched", () => {
  const now = new Date(2026, 10, 8, 12).getTime() / 1000;
  const at = (days, h = 12) => new Date(2026, 10, 8 - days, h).getTime() / 1000;
  const weekday = (t) => new Date(t * 1000).toLocaleDateString(undefined, { weekday: "short" });
  assert.equal(watchedWhen(at(0, 1), now), "today");
  assert.equal(watchedWhen(at(1, 23), now), "yesterday");
  assert.equal(watchedWhen(at(3), now), weekday(at(3)));
  assert.equal(watchedWhen(at(6), now), weekday(at(6)));
  assert.equal(watchedWhen(at(9), now), new Date(at(9) * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short" }));
  // another year: the full date
  const lastYear = new Date(2025, 9, 28, 12).getTime() / 1000;
  assert.match(watchedWhen(lastYear, now), /2025/);
  // a clock that is behind: still today
  assert.equal(watchedWhen(now + 600, now), "today");
});

test("the line under a title says how far, what is left and when", () => {
  setTimeFormat("24h");
  const now = new Date(2026, 10, 8, 12).getTime() / 1000;
  const yesterdayLate = new Date(2026, 10, 7, 22, 40).getTime() / 1000;
  const p = place(46 * 60 + 10, { duration: 121 * 60 + 25 });
  assert.equal(watchedLine(p, yesterdayLate, now), "46m watched · 1h 15m left · yesterday 22:40");
  assert.equal(watchedLine(p, null, now), "46m watched · 1h 15m left");
  // the clock follows every form of the when: a weekday, a date this year, another year
  const days = (n, h, m) => new Date(2026, 10, 8 - n, h, m).getTime() / 1000;
  for (const at of [days(3, 21, 5), days(9, 21, 5), new Date(2025, 9, 28, 21, 5).getTime() / 1000]) {
    assert.equal(watchedLine(p, at, now), `46m watched · 1h 15m left · ${watchedWhen(at, now)} 21:05`);
  }
  assert.match(watchedLine(p, days(9, 21, 5), now), /21:05$/);
  assert.equal(promptLine({ item: { kind: "movie", id: 2 }, title: "A Film", position: 2770, playedAt: yesterdayLate }, now), "You stopped at 46:10, yesterday at 22:40.");
});

test("where an open starts", () => {
  const p = place(1800);
  assert.deepEqual(startPlan("ask", p), { start: 1800, held: true, ask: true });
  assert.deepEqual(startPlan("ask", place(null)), { start: null, held: false, ask: false });
  assert.deepEqual(startPlan("ask", null), { start: null, held: false, ask: false });
  assert.deepEqual(startPlan("resume", p), { start: 1800, held: false, ask: false });
  assert.deepEqual(startPlan("resume", place(null)), { start: null, held: false, ask: false });
  assert.deepEqual(startPlan(null, p), { start: null, held: false, ask: false });   // Start over
  assert.deepEqual(startPlan(612, p), { start: 612, held: false, ask: false });     // a retry
  // the caller passes the fresh marks: a place written elsewhere wins over the row's
  assert.equal(startPlan("ask", { ...p, position: 2400 }).start, 2400);
});

test("the question shows while its title is held, and goes with it", () => {
  const pr = { item: { kind: "movie", id: 2 }, title: "A Film", position: 2770, playedAt: null };
  const at = (state, kind = "movie", id = 2) => ({ state, onDemand: kind ? { kind, id, position: 2770, duration: 7260, bufferedTo: null } : null });
  assert.deepEqual(promptStep(pr, false, at("playing", "episode", 9)), { shown: false, seen: false, clear: false }); // the open is on its way
  assert.deepEqual(promptStep(pr, false, at("opening")), { shown: true, seen: true, clear: false });
  assert.deepEqual(promptStep(pr, true, at("paused")), { shown: true, seen: true, clear: false });
  assert.equal(promptStep(pr, true, at("playing")).clear, true);           // Space, a media key, Resume
  assert.equal(promptStep(pr, true, at("failed")).clear, true);            // refused or failed
  assert.equal(promptStep(pr, true, at("stopped", null)).clear, true);     // Stop
  assert.equal(promptStep(pr, true, at("playing", "episode", 2)).clear, true); // episode 2 is another title
  assert.deepEqual(promptStep(null, false, at("paused")), { shown: false, seen: false, clear: false });
});

test("nothing clears a question until it has been seen", () => {
  const pr = { item: { kind: "movie", id: 2 }, title: "A Film", position: 2770, playedAt: null };
  const at = (state, kind = "movie", id = 2) => ({ state, onDemand: kind ? { kind, id, position: 2770, duration: 7260, bufferedTo: null } : null });
  // The same title already playing, or its failed open retried: stale state.
  assert.equal(promptStep(pr, false, at("playing")).clear, false);
  assert.equal(promptStep(pr, false, at("failed")).clear, false);
  assert.equal(promptStep(pr, false, at("stopped", null)).clear, false);
});

test("a question is seen for its own prompt only", () => {
  const a = { item: { kind: "movie", id: 1 }, title: "A", position: 100, playedAt: null };
  const c = { item: { kind: "movie", id: 3 }, title: "C", position: 100, playedAt: null };
  const at = (state, id) => ({ state, onDemand: { kind: "movie", id, position: 100, duration: 7200, bufferedTo: null } });
  assert.equal(seenFor(a, true, a), true);
  assert.equal(seenFor(a, true, c), false);
  assert.equal(seenFor(a, true, { ...a }), false);
  // A seen, then C asked while the player still reports A paused: not cleared, not shown; then C opening shows.
  const seen = seenFor(a, true, c);
  assert.deepEqual(promptStep(c, seen, at("paused", 1)), { shown: false, seen: false, clear: false });
  assert.deepEqual(promptStep(c, seen, at("opening", 3)), { shown: true, seen: true, clear: false });
});

test("the next episode starts where it was left, while in progress", () => {
  const marks = (o = {}) => ({ id: 8, favourite: false, watched: false, position: 60, duration: 120, playedAt: 1, ...o });
  assert.equal(nextStart(marks(), null), 60);
  assert.equal(nextStart(marks({ watched: true }), null), 0);
  assert.equal(nextStart(null, null), 0);
  assert.equal(nextStart(marks({ position: 0.5 }), null), 0);           // under 1 %
  assert.equal(nextStart(marks({ duration: null }), 120), 60);           // the listed length
  assert.equal(nextStart(marks({ duration: null }), null), 0);           // no length known
});

test("a reading opened again plays as the picture did, and a held title stays paused", () => {
  const at = (o) => ({ started: true, paused: false, ended: false, wantPlay: true, ...o });
  // Held for the question, before its first frame: it stays paused.
  assert.equal(playsOnReopen(at({ started: false, paused: true, wantPlay: false })), false);
  assert.equal(playsOnReopen(at({ started: false, paused: null, wantPlay: false })), false);
  // Not started yet, asked to play: plays.
  assert.equal(playsOnReopen(at({ started: false, paused: true, wantPlay: true })), true);
  assert.equal(playsOnReopen(at({})), true);                                       // playing
  assert.equal(playsOnReopen(at({ paused: true, wantPlay: true })), false);        // paused
  assert.equal(playsOnReopen(at({ paused: true, ended: true, wantPlay: false })), true); // ran out
  assert.equal(playsOnReopen(at({ paused: null, wantPlay: false })), true);        // no element
});

test("a held title's place is not reported while it is held, even as it is left", () => {
  const due = (o) =>
    placeReportDue({ held: false, final: false, last: null, now: 0, position: 2770, paused: true, ...o });
  // Looking at the question is not watching: no report, timed or final.
  assert.equal(due({ held: true }), false);
  assert.equal(due({ held: true, final: true }), false);
  assert.equal(due({ held: true, final: true, last: { at: 0, position: 12 } }), false);
  // Not held: as before.
  assert.equal(due({}), true);                                                   // first, past a second
  assert.equal(due({ position: 0.5 }), false);                                   // under a second, never reported
  assert.equal(due({ final: true, position: 0.5 }), false);
  assert.equal(due({ last: { at: 0, position: 2770 } }), false);                 // paused, not moved
  assert.equal(due({ last: { at: 0, position: 2700 } }), true);                  // paused after moving
  assert.equal(due({ final: true, last: { at: 0, position: 2770 } }), false);    // said already
  assert.equal(due({ final: true, last: { at: 0, position: 2700 } }), true);
  assert.equal(due({ paused: false, last: { at: 0, position: 2700 }, now: 9_000 }), false);
  assert.equal(due({ paused: false, last: { at: 0, position: 2700 }, now: 10_000 }), true);
});

test("a held title stays held until it plays", () => {
  assert.equal(heldAfter(true, "play"), false);     // Play, Resume, Space
  assert.equal(heldAfter(true, "playing"), false);  // its picture playing
  assert.equal(heldAfter(true, "seek"), true);      // a seek is not an answer
  assert.equal(heldAfter(true, "pause"), true);
  assert.equal(heldAfter(true, "reopen"), true);    // read again after a break
  assert.equal(heldAfter(false, "seek"), false);
  assert.equal(heldAfter(false, "pause"), false);
});

test("Up next stands aside for a held title until it has played", () => {
  const pr = { item: { kind: "episode", id: 7 }, title: "A Show · S1 E3", position: 109, playedAt: null };
  const at = (state, id = 7, kind = "episode") => ({ state, onDemand: id === null ? null : { kind, id, position: 109, duration: 120, bufferedTo: null } });
  // Asked: held from the open, before the player says anything of it.
  let h = holdStep(null, pr, at("playing", 4));
  assert.equal(holding(h), true);
  h = holdStep(h, pr, at("opening"));
  assert.equal(holding(h), true);
  h = holdStep(h, pr, at("paused"));
  assert.equal(holding(h), true);
  // Escape or a seek closes the question: still held, paused behind it.
  h = holdStep(h, null, at("paused"));
  assert.equal(holding(h), true);
  h = holdStep(h, null, at("opening"));                 // a seek read again
  assert.equal(holding(h), true);
  // It plays: the hold is over, and stays over.
  h = holdStep(h, null, at("playing"));
  assert.equal(holding(h), false);
  h = holdStep(h, null, at("paused"));
  assert.equal(holding(h), false);
  // The same prompt still in the store as it plays (cleared a render later) does not hold again.
  assert.equal(holding(holdStep(holdStep(holdStep(null, pr, at("paused")), pr, at("playing")), pr, at("paused"))), false);
});

test("a hold ends with its title, and a new question holds anew", () => {
  const pr = { item: { kind: "episode", id: 7 }, title: "A Show · S1 E3", position: 109, playedAt: null };
  const at = (state, id = 7, kind = "episode") => ({ state, onDemand: id === null ? null : { kind, id, position: 109, duration: 120, bufferedTo: null } });
  const seen = holdStep(null, pr, at("paused"));
  assert.equal(holding(holdStep(seen, null, at("stopped", null))), false);   // Stop
  assert.equal(holding(holdStep(seen, null, at("failed"))), false);          // refused
  assert.equal(holding(holdStep(seen, null, at("opening", 8))), false);      // another episode
  assert.equal(holding(holdStep(seen, null, at("playing", 7, "movie"))), false);
  // The question gone before it was ever seen (a failed open): no hold left behind.
  assert.equal(holding(holdStep(holdStep(null, pr, at("playing", 4)), null, at("failed", 4))), false);
  // Another question, even for the same title: held again, the title
  // reported playing from before the open changing nothing.
  const ended = holdStep(seen, null, at("playing"));
  const again = holdStep(ended, { ...pr }, at("playing"));
  assert.equal(holding(again), true);
  assert.equal(holding(holdStep(again, null, at("paused"))), true);
  assert.equal(holding(holdStep(ended, null, at("paused"))), false);
  assert.equal(holding(null), false);
});

test("a show's Play asks about a place it may have been given elsewhere", () => {
  // Its details say in progress: Resume, from the place as read now.
  assert.equal(showPlayStart(true), "resume");
  // Not in progress as far as they know: the place is read as it opens, and asked about if there is one.
  assert.equal(showPlayStart(false), "ask");
});
