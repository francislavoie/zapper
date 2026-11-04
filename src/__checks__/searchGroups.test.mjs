// Search results under their category: the ranking decides where each
// category sits, and every channel keeps its ranked place within it.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  NO_CATEGORY,
  categoryNames,
  categoryOf,
  categoryLine,
  groupByCategory,
} from "../logic/searchGroups.ts";

const ch = (streamId, categoryId, extra = {}) => ({
  streamId,
  name: `C${streamId}`,
  cleanName: `C${streamId}`,
  num: streamId,
  logoUrl: null,
  categoryId,
  hasArchive: false,
  archiveDays: null,
  isSeparator: false,
  quality: null,
  nowPlaying: null,
  ...extra,
});

const names = categoryNames([
  { id: "news", name: "UK | NEWS", channelCount: 3 },
  { id: "sport", name: "UK | SPORT", channelCount: 3 },
]);

const shape = (rows) => rows.map((r) => (r.isSeparator ? `# ${r.name}` : r.streamId));

test("categories come in the order of their best hit, channels in rank order", () => {
  const rows = groupByCategory([ch(1, "sport"), ch(2, "news"), ch(3, "sport"), ch(4, "news")], names);
  assert.deepEqual(shape(rows), ["# UK | SPORT", 1, 3, "# UK | NEWS", 2, 4]);
});

test("headings are separators with ids no channel has", () => {
  const rows = groupByCategory([ch(1, "sport"), ch(2, "news")], names);
  const heads = rows.filter((r) => r.isSeparator);
  assert.equal(heads.length, 2);
  assert.ok(heads.every((h) => h.streamId < 0));
  assert.equal(new Set(rows.map((r) => r.streamId)).size, rows.length);
});

test("a channel in no category, or one not yet known, is filed under no category", () => {
  const rows = groupByCategory([ch(1, null), ch(2, "gone")], names);
  assert.deepEqual(shape(rows), [`# ${NO_CATEGORY}`, 1, 2]);
  assert.equal(categoryOf(names, "news"), "UK | NEWS");
  assert.equal(categoryOf(names, null), NO_CATEGORY);
});

test("nothing found is nothing listed, and separators are never results", () => {
  assert.deepEqual(groupByCategory([], names), []);
  assert.deepEqual(shape(groupByCategory([ch(9, "news", { isSeparator: true })], names)), []);
});

test("a row leaves out its category under a divider of the same category", () => {
  assert.equal(categoryLine("cat-1", "cat-1", "CA | ENTERTAINMENT"), null);
});

test("a row keeps its category under another category's divider, or none", () => {
  // Same topic, different country: the ids differ, so the line stays.
  assert.equal(categoryLine("ca-news", "uk-news", "UK | NEWS"), "UK | NEWS");
  assert.equal(categoryLine(null, "cat-1", "CA | ENTERTAINMENT"), "CA | ENTERTAINMENT");
  assert.equal(categoryLine(null, null, null), null);
  assert.equal(categoryLine("cat-1", "cat-1", null), null);
});
