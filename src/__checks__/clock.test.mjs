// Times of day on either clock: the 12/24-hour setting, what "auto" makes of
// the locale, and each helper's output in both forms. Instants are built in
// local time, so the checks hold in any zone.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  clockIs12h,
  dateTime,
  hhmm,
  setTimeFormat,
  systemUses12h,
  timeRange,
  timeTick,
  uses12h,
} from "../logic/format.ts";
import { DEFAULT_SETTINGS, clampSettings } from "../logic/settings.ts";

const at = (h, m = 0) => new Date(2026, 10, 1, h, m).getTime() / 1000;

test("noon, midnight and the hours either side, on both clocks", () => {
  assert.equal(hhmm(at(12), true), "12:00 pm");
  assert.equal(hhmm(at(12), false), "12:00");
  assert.equal(hhmm(at(0, 30), true), "12:30 am");
  assert.equal(hhmm(at(0, 30), false), "00:30");
  assert.equal(hhmm(at(11, 59), true), "11:59 am");
  assert.equal(hhmm(at(13, 5), true), "1:05 pm");
  assert.equal(hhmm(at(20, 5), false), "20:05");
  assert.equal(hhmm(at(8, 5), false), "08:05");
});

test("a range says am/pm once when both ends share it", () => {
  assert.equal(timeRange(at(20), at(21, 30), true), "8:00–9:30 pm");
  assert.equal(timeRange(at(23, 30), at(0, 30), true), "11:30 pm–12:30 am");
  assert.equal(timeRange(at(11), at(12), true), "11:00 am–12:00 pm");
  assert.equal(timeRange(at(20), at(21, 30), false), "20:00–21:30");
});

test("axis ticks are compact on the 12-hour clock", () => {
  assert.equal(timeTick(at(20), true), "8 pm");
  assert.equal(timeTick(at(20, 30), true), "8:30");
  assert.equal(timeTick(at(0), true), "12 am");
  assert.equal(timeTick(at(12), true), "12 pm");
  assert.equal(timeTick(at(20), false), "20:00");
  assert.equal(timeTick(at(20, 30), false), "20:30");
});

test("a date and time ends with the time as set", () => {
  assert.match(dateTime(at(20, 5), true), /, 8:05 pm$/);
  assert.match(dateTime(at(20, 5), false), /, 20:05$/);
});

test("auto follows the locale; the overrides do not", () => {
  assert.equal(systemUses12h("en-US"), true);
  assert.equal(systemUses12h("en-GB"), false);
  assert.equal(systemUses12h("fr-CA"), false);
  assert.equal(uses12h("auto", true), true);
  assert.equal(uses12h("auto", false), false);
  assert.equal(uses12h("12h", false), true);
  assert.equal(uses12h("24h", true), false);
  // Unknown is auto, so a hand-edited config cannot break the clock.
  assert.equal(uses12h("sundial", true), true);
});

test("the setting reaches the helpers' default", () => {
  setTimeFormat("12h");
  assert.equal(clockIs12h(), true);
  assert.equal(hhmm(at(12)), "12:00 pm");
  setTimeFormat("24h");
  assert.equal(clockIs12h(), false);
  assert.equal(hhmm(at(12)), "12:00");
});

test("the setting defaults to auto and keeps only what it knows", () => {
  assert.equal(DEFAULT_SETTINGS.timeFormat, "auto");
  const old = { ...DEFAULT_SETTINGS };
  delete old.timeFormat;
  assert.equal(clampSettings(old).timeFormat, "auto");
  assert.equal(clampSettings({ ...DEFAULT_SETTINGS, timeFormat: "12h" }).timeFormat, "12h");
  assert.equal(clampSettings({ ...DEFAULT_SETTINGS, timeFormat: "24h" }).timeFormat, "24h");
  assert.equal(clampSettings({ ...DEFAULT_SETTINGS, timeFormat: "25h" }).timeFormat, "auto");
});
