import test from "node:test";
import assert from "node:assert/strict";
import {
  PROGRAMME_MIN_LEAD_S,
  SLEEP_FADE_MS,
  SLEEP_MINUTES,
  fadedVolume,
  isRequest,
  programmeForSleep,
  remainingLabel,
  remainingMs,
  requestLabel,
  sleepStep,
  startSleep,
} from "../logic/sleep.ts";

const T0 = 1_800_000_000_000; // ms
const prog = (start, stop, title = "News") => ({ start, stop, title, description: null });

test("the lengths on offer are the five the menus list", () => {
  assert.deepEqual([...SLEEP_MINUTES], [15, 30, 45, 60, 90]);
});

test("a length counts from when it was set", () => {
  const t = startSleep(30, T0, null);
  assert.equal(t.endsAt, T0 + 30 * 60_000);
  assert.equal(t.programme, null);
  assert.equal(remainingMs(t, T0 + 60_000), 29 * 60_000);
  assert.equal(remainingMs(t, T0 + 31 * 60_000), 0);
  assert.equal(startSleep(0, T0, null), null);
});

test("the end of the programme is the guide's end, and only while far enough away", () => {
  const stop = T0 / 1000 + 20 * 60;
  const t = startSleep("programme", T0, prog(T0 / 1000 - 600, stop, "Film"));
  assert.equal(t.endsAt, stop * 1000);
  assert.equal(t.programme, "Film");
  assert.equal(startSleep("programme", T0, null), null);
  const soon = T0 / 1000 + PROGRAMME_MIN_LEAD_S - 1;
  assert.equal(startSleep("programme", T0, prog(T0 / 1000 - 600, soon)), null);
});

test("the timer waits, then fades over the last ten seconds, then expires", () => {
  const t = startSleep(15, T0, null);
  assert.deepEqual(sleepStep(t, T0), { phase: "waiting" });
  assert.deepEqual(sleepStep(t, t.endsAt - SLEEP_FADE_MS - 1), { phase: "waiting" });
  const half = sleepStep(t, t.endsAt - SLEEP_FADE_MS / 2);
  assert.equal(half.phase, "fading");
  assert.ok(Math.abs(half.level - 0.5) < 1e-9);
  assert.deepEqual(sleepStep(t, t.endsAt), { phase: "expired" });
  assert.deepEqual(sleepStep(t, t.endsAt + 5000), { phase: "expired" });
  assert.equal(SLEEP_FADE_MS, 10_000);
});

test("the fade runs from the starting volume down to silence, never up", () => {
  assert.equal(fadedVolume(80, 1), 80);
  assert.equal(fadedVolume(80, 0.5), 40);
  assert.equal(fadedVolume(80, 0), 0);
  assert.equal(fadedVolume(80, 1.5), 80);
  assert.equal(fadedVolume(80, -1), 0);
  assert.equal(fadedVolume(150, 1 / 3), 50);
  // Steps of a tenth: nothing finer is worth a round trip to the player.
  assert.equal(fadedVolume(33, 0.333), 11);
});

test("time left is said briefly, rounding minutes up", () => {
  assert.equal(remainingLabel(30 * 60_000), "30m");
  assert.equal(remainingLabel(30 * 60_000 - 1000), "30m");
  assert.equal(remainingLabel(29 * 60_000), "29m");
  assert.equal(remainingLabel(61_000), "2m");
  assert.equal(remainingLabel(60_000), "1m");
  assert.equal(remainingLabel(42_000), "42s");
  assert.equal(remainingLabel(41_200), "42s");
  assert.equal(remainingLabel(90 * 60_000), "1h30m");
  assert.equal(remainingLabel(65 * 60_000), "1h05m");
  assert.equal(remainingLabel(-5), "0s");
});

test("requests have names and compare by what was asked for", () => {
  assert.equal(requestLabel(45), "45 minutes");
  assert.equal(requestLabel("programme"), "End of this programme");
  const t = startSleep(45, T0, null);
  assert.ok(isRequest(t, 45));
  assert.ok(!isRequest(t, 30));
  assert.ok(!isRequest(null, 45));
});

test("only a live programme on now, ending far enough away, can end the timer", () => {
  const now = T0 / 1000;
  const on = prog(now - 600, now + 1800);
  assert.equal(programmeForSleep(on, null, now), on);
  assert.equal(programmeForSleep(null, null, now), null);
  // Not started yet.
  assert.equal(programmeForSleep(prog(now + 60, now + 1800), null, now), null);
  // About to end.
  assert.equal(programmeForSleep(prog(now - 600, now + 30), null, now), null);
  // Watching a catch-up of something that already aired: the guide's "now"
  // is not what is on screen.
  assert.equal(programmeForSleep(on, prog(now - 7200, now - 3600), now), null);
  // Opened for the programme that is on now: that is live.
  assert.equal(programmeForSleep(on, on, now), on);
});
