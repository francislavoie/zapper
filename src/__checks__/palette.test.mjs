import test from "node:test";
import assert from "node:assert/strict";
import {
  ACTIONS,
  actionMatches,
  availableActions,
  clampIndex,
  filterActions,
  moveIndex,
  paletteItems,
} from "../logic/palette.ts";

const chan = (streamId, cleanName, isSeparator = false) => ({
  streamId,
  name: cleanName,
  cleanName,
  num: streamId,
  logoUrl: null,
  categoryId: "c",
  hasArchive: false,
  archiveDays: null,
  isSeparator,
  quality: null,
  nowPlaying: null,
});

test("an empty query matches every action", () => {
  assert.equal(filterActions(ACTIONS, "").length, ACTIONS.length);
  assert.equal(filterActions(ACTIONS, "   ").length, ACTIONS.length);
});

test("action filtering is subsequence, not just prefix", () => {
  assert.ok(actionMatches("Toggle compressor", "tc"));
  assert.ok(actionMatches("Toggle compressor", "compress"));
  assert.ok(actionMatches("Toggle compressor", "TOGGLE"));
  assert.ok(!actionMatches("Toggle compressor", "zz"));
  // Order matters: a subsequence is not an anagram.
  assert.ok(!actionMatches("Toggle compressor", "ct t o g"));
});

test("the hint is searchable, because that is the word people type", () => {
  // "sync" is nowhere in "Refresh data from the provider", but it is the hint.
  assert.ok(!actionMatches("Refresh data from the provider", "sync"));
  assert.ok(actionMatches("Refresh data from the provider", "sync", "sync"));
  assert.equal(filterActions(ACTIONS, "sync").length, 1);
  assert.equal(filterActions(ACTIONS, "sync")[0].id, "sync");
});

test("player-only actions are hidden when nothing is playing", () => {
  const idle = availableActions(false).map((a) => a.id);
  assert.ok(!idle.includes("compressor"));
  assert.ok(!idle.includes("live"));
  assert.ok(idle.includes("sync"));
  assert.ok(idle.includes("settings"));

  const live = availableActions(true).map((a) => a.id);
  assert.ok(live.includes("compressor"));
  assert.equal(live.length, ACTIONS.length);
});

test("actions come before channels", () => {
  const items = paletteItems(filterActions(ACTIONS, "s"), [chan(1, "MERIDIAN")]);
  assert.equal(items[0].kind, "action");
  assert.equal(items[items.length - 1].kind, "channel");
});

test("separators never reach the palette", () => {
  const items = paletteItems([], [chan(1, "MERIDIAN"), chan(2, "SPORTS", true), chan(3, "SILVERLINE")]);
  assert.equal(items.length, 2);
  assert.ok(items.every((i) => i.kind === "channel" && !i.channel.isSeparator));
});

test("clampIndex keeps the selection inside the list as it shrinks", () => {
  assert.equal(clampIndex(9, 3), 2);
  assert.equal(clampIndex(-4, 3), 0);
  assert.equal(clampIndex(1, 3), 1);
  // An empty list has no valid index, and zero is the safe answer.
  assert.equal(clampIndex(5, 0), 0);
});

test("arrow movement wraps in both directions", () => {
  assert.equal(moveIndex(0, 1, 3), 1);
  assert.equal(moveIndex(2, 1, 3), 0);
  assert.equal(moveIndex(0, -1, 3), 2);
  assert.equal(moveIndex(1, -1, 3), 0);
  assert.equal(moveIndex(0, 1, 0), 0);
  // A big jump (page down) still lands in range.
  assert.equal(moveIndex(0, 10, 4), 2);
  assert.equal(moveIndex(0, -10, 4), 2);
});
