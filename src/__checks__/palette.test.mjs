import test from "node:test";
import assert from "node:assert/strict";
import {
  ACTIONS,
  actionMatches,
  availableActions,
  clampIndex,
  filterActions,
  groupHeading,
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

  const live = availableActions(true, { hasPrevious: true, sleeping: true, programme: true }).map(
    (a) => a.id,
  );
  assert.ok(live.includes("compressor"));
  // Every action but signing out, which only a browser has to do.
  assert.equal(live.length, ACTIONS.length - 1);
});

test("the previous channel is offered only when there is one", () => {
  assert.ok(!availableActions(true).some((a) => a.id === "previous"));
  assert.ok(availableActions(true, { hasPrevious: true }).some((a) => a.id === "previous"));
  // Not a player control: after a stop, going back is still on offer.
  assert.ok(availableActions(false, { hasPrevious: true }).some((a) => a.id === "previous"));
});

test("actions come before channels", () => {
  const items = paletteItems(filterActions(ACTIONS, "s"), [chan(1, "MERIDIAN")]);
  assert.equal(items[0].kind, "action");
  assert.equal(items[items.length - 1].kind, "channel");
});

const hit = (streamId, title, airing = "upcoming") => ({
  channel: chan(streamId, `CH ${streamId}`),
  programme: { start: 1000, stop: 2000, title, description: null },
  airing,
});

test("programmes follow the channels, in the order they came", () => {
  const items = paletteItems(
    filterActions(ACTIONS, "s"),
    [chan(1, "MERIDIAN")],
    [hit(2, "Harbour Lights", "live"), hit(3, "Harbour Lights")],
  );
  const kinds = items.map((i) => i.kind);
  assert.equal(kinds.indexOf("channel"), kinds.lastIndexOf("action") + 1);
  assert.deepEqual(kinds.slice(-2), ["programme", "programme"]);
  assert.deepEqual(
    items.filter((i) => i.kind === "programme").map((i) => i.hit.airing),
    ["live", "upcoming"],
  );
  // Without programmes, the list is what it always was.
  assert.equal(paletteItems([], [chan(1, "M")]).length, 1);
});

test("search groups are headed where they start, actions are not", () => {
  const items = paletteItems(
    filterActions(ACTIONS, "s").slice(0, 2),
    [chan(1, "A"), chan(2, "B")],
    [hit(3, "Show"), hit(4, "Show")],
  );
  const headings = items.map((_, i) => groupHeading(items, i));
  assert.deepEqual(headings, [null, null, "Channels", null, "Programmes", null]);
  // Programmes straight after the actions still get their heading.
  const noChannels = paletteItems(ACTIONS.slice(0, 1), [], [hit(3, "Show")]);
  assert.equal(groupHeading(noChannels, 1), "Programmes");
  assert.equal(groupHeading(noChannels, 5), null);
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

// ------------------------------------------------------------- sleep timer

test("every sleep length is its own palette entry, findable by typing it", () => {
  const live = availableActions(true);
  for (const m of [15, 30, 45, 60, 90]) {
    const hits = filterActions(live, `sleep ${m}`);
    assert.ok(hits.some((a) => a.id === `sleep${m}` && a.sleep === m), `sleep ${m}`);
  }
});

test("the sleep actions need something playing", () => {
  const idle = availableActions(false, { sleeping: true, programme: true }).map((a) => a.id);
  assert.ok(idle.every((id) => !id.startsWith("sleep")));
});

test("'end of this programme' needs the guide to know the end, and 'off' needs a timer", () => {
  const ids = (c) => availableActions(true, c).map((a) => a.id);
  assert.ok(!ids({}).includes("sleepProgramme"));
  assert.ok(ids({ programme: true }).includes("sleepProgramme"));
  assert.ok(!ids({}).includes("sleepOff"));
  assert.ok(ids({ sleeping: true }).includes("sleepOff"));
  // The palette's words: what the hamburger's submenu calls just "Off".
  const off = ACTIONS.find((a) => a.id === "sleepOff");
  assert.equal(off.label, "Sleep timer: off");
  assert.equal(off.short, "Off");
  assert.equal(off.sleep, null);
});
