// Where a title was left: when it counts as in progress, what is said of it,
// and where an open starts. Run: node --test src/__checks__/

import assert from "node:assert/strict";
import { test } from "node:test";
import { setTimeFormat } from "../logic/format.ts";
import {
  progressOf,
  promptLine,
  promptStep,
  resumeLabel,
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
