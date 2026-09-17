// What a favourite says about itself. All of it is `Intl`, and all of it is
// pure, so none of it needs eyes.
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  favouriteMeta,
  lastWatchedLabel,
  menuPosition,
  watchTimeLabel,
  watchedLabel,
} from "../logic/favourites.ts";

// en-US, which is what this machine runs and what the abbreviations below
// are. Other English locales differ — en-GB pluralises them to "5 mins" —
// and that is Intl being right rather than this being wrong; the test two
// below pins that the labels come from Intl at all.
const EN = "en-US";
const HOUR = 3600;
const DAY = 86_400;

// --------------------------------------------------------------- watch time

test("under a minute is said in seconds, in whole words", () => {
  assert.equal(watchTimeLabel(45, EN), "45 sec");
  assert.equal(watchTimeLabel(1, EN), "1 sec");
});

test("under an hour is said in minutes", () => {
  assert.equal(watchTimeLabel(12 * 60, EN), "12 min");
  assert.equal(watchTimeLabel(60, EN), "1 min");
  // 59:59 is still 59 minutes: a channel must not claim a minute it has not
  // been watched.
  assert.equal(watchTimeLabel(3599, EN), "59 min");
});

test("an hour and over is the compact form the rail has room for", () => {
  assert.equal(watchTimeLabel(4 * HOUR + 20 * 60, EN), "4h 20m");
  assert.equal(watchTimeLabel(HOUR, EN), "1h");
  // A whole number of hours does not say "and zero minutes".
  assert.equal(watchTimeLabel(3 * HOUR, EN), "3h");
  assert.equal(watchTimeLabel(46 * HOUR, EN), "46h");
});

test("nonsense in is a label out, not a crash or an empty string", () => {
  for (const bad of [0, -1, Number.NaN, Number.POSITIVE_INFINITY, undefined]) {
    const s = watchTimeLabel(bad, EN);
    assert.equal(typeof s, "string");
    assert.ok(s.length > 0, `empty label for ${String(bad)}`);
  }
  assert.equal(watchTimeLabel(0, EN), "0 sec");
});

test("the units come from Intl rather than from a hand-rolled table", () => {
  // The point of going through Intl is that another locale is a catalogue
  // drop-in rather than a rewrite. French says minutes differently, and if
  // these were hard-coded strings this assertion could not pass.
  assert.notEqual(watchTimeLabel(12 * 60, "fr-FR"), watchTimeLabel(12 * 60, "ja-JP"));
  assert.notEqual(watchTimeLabel(45, "fr-FR"), watchTimeLabel(45, "en-GB"));
});

// ------------------------------------------------------------ last watched

test("just watched reads as now, not as zero seconds ago", () => {
  assert.equal(lastWatchedLabel(1000, 1000, EN), "now");
  assert.equal(lastWatchedLabel(1000, 1030, EN), "now");
});

test("each scale is said in its own unit", () => {
  const now = 1_700_000_000;
  assert.equal(lastWatchedLabel(now - 20 * 60, now, EN), "20 min ago");
  assert.equal(lastWatchedLabel(now - 5 * HOUR, now, EN), "5 hr ago");
  // "auto" is what turns one day into a word.
  assert.equal(lastWatchedLabel(now - DAY, now, EN), "yesterday");
  assert.equal(lastWatchedLabel(now - 3 * DAY, now, EN), "3 days ago");
  assert.equal(lastWatchedLabel(now - 60 * DAY, now, EN), "2 mo ago");
  assert.equal(lastWatchedLabel(now - 800 * DAY, now, EN), "2 yr ago");
});

test("never watched says nothing at all, and the caller decides what that means", () => {
  assert.equal(lastWatchedLabel(null, 1000, EN), "");
  assert.equal(lastWatchedLabel(Number.NaN, 1000, EN), "");
});

test("a timestamp in the future reads as now rather than as a countdown", () => {
  // Clocks change, and the database outlives any one boot. The backend's score
  // clamps the same way.
  const now = 1_700_000_000;
  assert.equal(lastWatchedLabel(now + 500, now, EN), "now");
  assert.equal(lastWatchedLabel(now + 400 * DAY, now, EN), "now");
});

// -------------------------------------------------------------------- meta

test("the line under a name is what it earned and when", () => {
  const now = 1_700_000_000;
  assert.equal(
    favouriteMeta(4 * HOUR + 20 * 60, now - 3 * DAY, now, EN),
    "4h 20m · 3 days ago",
  );
  // With no timestamp there is no separator left dangling.
  assert.equal(favouriteMeta(12 * 60, null, now, EN), "12 min");
});

// --------------------------------------------------------------- menu box

test("a context menu opens down and to the right of the pointer", () => {
  assert.deepEqual(menuPosition(100, 200, 220, 120, 1200, 800), { left: 100, top: 200 });
});

test("a menu near an edge flips back inside the window instead of running off it", () => {
  const vw = 1200;
  const vh = 800;
  const box = menuPosition(1150, 760, 220, 120, vw, vh, 6);
  assert.equal(box.left, vw - 220 - 6);
  assert.equal(box.top, vh - 120 - 6);
  assert.ok(box.left + 220 <= vw, "menu runs off the right");
  assert.ok(box.top + 120 <= vh, "menu runs off the bottom");
});

test("a menu bigger than the window still starts inside it", () => {
  // The window is undecorated: its outer few pixels are the resize grab, so
  // the margin is a floor as well as a ceiling.
  const box = menuPosition(10, 10, 4000, 4000, 300, 200, 6);
  assert.equal(box.left, 6);
  assert.equal(box.top, 6);
});

test("the pointer's own position is honoured when there is room for it", () => {
  for (const [x, y] of [
    [0, 0],
    [1, 1],
    [400, 300],
  ]) {
    const box = menuPosition(x, y, 200, 100, 1000, 1000, 6);
    assert.ok(box.left >= 6 && box.top >= 6);
    assert.ok(box.left <= Math.max(6, x), `moved right of the pointer at ${x}`);
  }
});

test("the watch time says what it is a measure of", () => {
  // A bare duration beside a channel name could as easily be how long the
  // programme runs; this column is why the channel is in the list.
  assert.equal(watchedLabel(5 * 60, EN), "Watched 5 min");
  assert.equal(watchedLabel(0, EN), "Watched 0 sec");
});

test("abbreviated relative times do not keep their full stop", () => {
  // Intl's short style says "10 min. ago", and the stop reads as a typo
  // beside "Watched 5 min", which has none because it comes from a
  // different formatter.
  const now = 1_757_000_000;
  for (const ago of [20 * 60, 5 * HOUR, 3 * DAY, 60 * DAY, 800 * DAY]) {
    const said = lastWatchedLabel(now - ago, now, EN);
    assert.ok(!said.includes("."), `"${said}" kept a full stop`);
    assert.ok(said.endsWith(" ago"), said);
  }
});
