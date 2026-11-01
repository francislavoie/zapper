// Settings backups: the file format, checking a file someone hands us, and
// saying what importing it would change before anything does.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  BACKUP_FORMAT,
  BACKUP_VERSION,
  backupFileName,
  buildBackup,
  mergeActivity,
  parseBackup,
  planHasChanges,
  planImport,
} from "../logic/backup.ts";
import { DEFAULT_SETTINGS } from "../logic/settings.ts";

const categories = { pins: ["uk", "sports"], hidden: ["adult"], sort: "provider" };
const row = (streamId, watchSeconds, lastWatched = 1_000, openCount = 1) => ({
  streamId,
  name: `Channel ${streamId}`,
  watchSeconds,
  lastWatched,
  openCount,
});
const here = (p = {}) => ({
  settings: DEFAULT_SETTINGS,
  categories,
  activity: [row(1, 600, 5_000, 3)],
  ...p,
});
const ok = (doc) => {
  const r = parseBackup(doc);
  assert.equal(r.ok, true, r.error);
  return r;
};

test("an export reads back as exactly what was exported", () => {
  const doc = buildBackup(
    { settings: DEFAULT_SETTINGS, categories, favourites: [row(1, 600)] },
    new Date("2026-11-01T12:00:00Z"),
  );
  assert.equal(doc.format, BACKUP_FORMAT);
  assert.equal(doc.version, BACKUP_VERSION);
  assert.equal(doc.exportedAt, "2026-11-01T12:00:00.000Z");
  assert.equal("account" in doc, false, "the account is the backend's to add");
  // Through JSON, as it goes through a file.
  const { backup, warnings } = ok(JSON.parse(JSON.stringify(doc)));
  assert.deepEqual(warnings, []);
  assert.deepEqual(backup.settings, DEFAULT_SETTINGS);
  assert.deepEqual(backup.categories, categories);
  assert.deepEqual(backup.favourites, [row(1, 600)]);
  const plan = planImport(here(), backup);
  assert.deepEqual(planHasChanges(plan), { settings: false, categories: false, favourites: false });
});

test("the file is named for the day, in local time", () => {
  assert.equal(backupFileName(new Date(2026, 0, 5, 23, 59)), "zapper-settings-2026-01-05.json");
});

test("a file that is not a backup, or from a newer zapper, is refused", () => {
  for (const doc of [null, [], "x", {}, { format: "other", version: 1 }]) {
    assert.match(parseBackup(doc).error, /not a zapper backup/);
  }
  assert.match(parseBackup({ format: BACKUP_FORMAT }).error, /which version/);
  assert.match(parseBackup({ format: BACKUP_FORMAT, version: 1.5 }).error, /which version/);
  const newer = parseBackup({ format: BACKUP_FORMAT, version: BACKUP_VERSION + 1 });
  assert.match(newer.error, /newer zapper/);
});

test("every section is optional, and an absent one changes nothing", () => {
  const { backup } = ok({ format: BACKUP_FORMAT, version: 1 });
  assert.equal(backup.settings, null);
  assert.equal(backup.categories, null);
  assert.equal(backup.favourites, null);
  assert.deepEqual(planImport(here(), backup), { settings: null, categories: null, favourites: null });
});

test("a wrongly typed setting is skipped and said, the rest still imported", () => {
  const { backup, warnings } = ok({
    format: BACKUP_FORMAT,
    version: 1,
    settings: {
      guideHours: "six",
      rewindMb: 1024,
      mediaControls: true,
      warpDrive: true,
      subtitle: { size: 52, backdrop: "neon", color: 7 },
    },
  });
  assert.deepEqual(warnings, [
    "Skipped guideHours: not a number.",
    "Skipped subtitle.color: not a string.",
    'Skipped subtitle.backdrop: "neon" is not one zapper draws.',
  ]);
  const plan = planImport(here(), backup);
  // Missing and skipped fields keep what is here; the rest is clamped as any
  // other setting is.
  assert.equal(plan.settings.next.guideHours, DEFAULT_SETTINGS.guideHours);
  assert.equal(plan.settings.next.subtitle.color, DEFAULT_SETTINGS.subtitle.color);
  assert.deepEqual(
    plan.settings.changes.map((c) => [c.label, c.from, c.to]),
    [
      ["Rewind buffer", "512 MB", "1,024 MB"],
      ["Media keys and desktop controls", "off", "on"],
      ["Subtitle size", "38pt", "52pt"],
    ],
  );
});

test("the clock travels in a backup, named as Settings names it", () => {
  const doc = buildBackup(
    { settings: { ...DEFAULT_SETTINGS, timeFormat: "12h" }, categories, favourites: [] },
    new Date("2026-11-01T12:00:00Z"),
  );
  const { backup } = ok(JSON.parse(JSON.stringify(doc)));
  assert.equal(backup.settings.timeFormat, "12h");
  const plan = planImport(here(), backup);
  assert.deepEqual(
    plan.settings.changes.map((c) => [c.label, c.from, c.to]),
    [["Clock", "System", "12-hour"]],
  );
  // One this build does not know reads as the system's, and changes nothing.
  const odd = ok({ format: BACKUP_FORMAT, version: 1, settings: { timeFormat: "36h" } });
  assert.equal(planImport(here(), odd.backup).settings.next.timeFormat, "auto");
  assert.deepEqual(planImport(here(), odd.backup).settings.changes, []);
});

test("imported settings are clamped like any other", () => {
  const { backup } = ok({
    format: BACKUP_FORMAT,
    version: 1,
    settings: { rewindMb: 999_999, guideHours: 0 },
  });
  const next = planImport(here(), backup).settings.next;
  assert.equal(next.rewindMb, 2000);
  assert.equal(next.guideHours, 1);
});

test("category choices replace what is here, and say how", () => {
  const { backup, warnings } = ok({
    format: BACKUP_FORMAT,
    version: 1,
    categories: { pins: ["sports", "uk", "uk", 4], hidden: [], sort: "az" },
  });
  assert.deepEqual(warnings, ["Dropped unreadable or repeated entries from pinned categories."]);
  const plan = planImport(here(), backup);
  assert.deepEqual(plan.categories.next, { pins: ["sports", "uk"], hidden: [], sort: "az" });
  assert.deepEqual(
    plan.categories.changes.map((c) => [c.label, c.from, c.to]),
    [
      ["Pinned categories", "2 categories", "2 categories, reordered"],
      ["Hidden categories", "1 category", "0 categories"],
      ["Category order", "Provider's order", "A–Z"],
    ],
  );
  const bad = ok({ format: BACKUP_FORMAT, version: 1, categories: { sort: "random" } });
  assert.match(bad.warnings[0], /category sort/);
  assert.deepEqual(planImport(here(), bad.backup).categories.changes, []);
});

test("watch history merges by the larger figure, so importing twice is once", () => {
  const local = row(1, 600, 5_000, 3);
  assert.deepEqual(mergeActivity(local, row(1, 300, 9_000, 1)), row(1, 600, 9_000, 3));
  assert.deepEqual(mergeActivity(local, row(1, 900, null, 1)), row(1, 900, 5_000, 3));
  assert.deepEqual(
    mergeActivity({ ...local, lastWatched: null }, row(1, 0, 7, 0)),
    { ...local, lastWatched: 7 },
  );

  const { backup } = ok({
    format: BACKUP_FORMAT,
    version: 1,
    favourites: [row(1, 300, 9_000), row(2, 60), row(3, 0, null, 0)],
  });
  const fav = planImport(here(), backup).favourites;
  assert.equal(fav.added, 1, "channel 2; channel 3 has nothing to add");
  assert.equal(fav.raised, 1, "channel 1's later date");
  const again = planImport(here({ activity: [row(1, 600, 9_000, 3), row(2, 60)] }), backup);
  assert.equal(planHasChanges(again).favourites, false);
});

test("unreadable watch history is skipped and counted; a channel listed twice is merged", () => {
  const { backup, warnings } = ok({
    format: BACKUP_FORMAT,
    version: 1,
    favourites: [
      row(1, 60),
      row(1, 120, 500, 4),
      { streamId: "2", watchSeconds: 5 },
      { streamId: 3, watchSeconds: -1 },
      { streamId: 4, watchSeconds: 5, lastWatched: 1.5 },
      { streamId: 5, watchSeconds: 5 },
      null,
    ],
  });
  assert.deepEqual(warnings, ["Skipped 4 unreadable watch-history entries."]);
  assert.deepEqual(backup.favourites, [
    row(1, 120, 1_000, 4),
    { streamId: 5, name: null, watchSeconds: 5, lastWatched: null, openCount: 0 },
  ]);
});
