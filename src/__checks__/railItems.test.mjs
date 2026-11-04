import test from "node:test";
import assert from "node:assert/strict";
import { ALL_CATEGORIES as ALL, FAVOURITES, RECENT, RECORDINGS, railItemKind, pagedCategory,
  guideSelection, modeSelection, selectionTitle, selectionSubtitle, selectionEstimate, listedFor,
  focusTarget, fixedRailItems } from "../logic/railItems.ts";
const CATS = [{ id: "uk", name: "UK | News", channelCount: 40 }, { id: "sp", name: "Sport", channelCount: 12 }];

test("the rail's own items are told apart from categories", () => {
  assert.deepEqual([FAVOURITES, RECENT, RECORDINGS, ALL, "uk"].map(railItemKind),
    ["favourites", "recent", "recordings", "all", "category"]);
});
test("only All channels and categories page from the backend", () => {
  assert.equal(pagedCategory(ALL), null);
  assert.equal(pagedCategory("uk"), "uk");
  for (const id of [FAVOURITES, RECENT, RECORDINGS]) assert.equal(pagedCategory(id), undefined);
});
test("Recordings has no guide of its own: asking for one shows every channel", () => {
  assert.equal(guideSelection(RECORDINGS), ALL);
  for (const id of [FAVOURITES, RECENT, ALL, "uk"]) assert.equal(guideSelection(id), id);
  assert.equal(modeSelection(RECORDINGS, "guide"), ALL);
  assert.equal(modeSelection(RECORDINGS, "browse"), RECORDINGS);
  assert.equal(modeSelection(FAVOURITES, "guide"), FAVOURITES);
});
test("each selection has a title and says how many it holds", () => {
  assert.equal(selectionTitle(FAVOURITES, CATS), "Favourites");
  assert.equal(selectionTitle(RECENT, CATS), "Recently watched");
  assert.equal(selectionTitle(RECORDINGS, CATS), "Recordings");
  assert.equal(selectionTitle(ALL, CATS), "All channels");
  assert.equal(selectionTitle("uk", CATS), "UK | News");
  assert.equal(selectionTitle("gone", CATS), "Channels");
  assert.equal(selectionSubtitle(FAVOURITES, 18), "18 channels, most watched first");
  assert.equal(selectionSubtitle(RECENT, 1), "1 channel, newest first");
  assert.equal(selectionSubtitle(ALL, 49526), `${(49526).toLocaleString()} channels`);
  assert.equal(selectionEstimate(ALL, CATS, 999), 52);
  assert.equal(selectionEstimate("sp", CATS, 999), 12);
  assert.equal(selectionEstimate(FAVOURITES, CATS, 999), 0);
});
test("going to a channel keeps the list on screen when the channel is in it", () => {
  const ch = { streamId: 7, categoryId: "sp" };
  assert.deepEqual(focusTarget(FAVOURITES, ch, [3, 7]), { categoryId: FAVOURITES, index: 1, lookUp: false });
  assert.deepEqual(focusTarget(FAVOURITES, ch, [3]), { categoryId: "sp", index: null, lookUp: true });
  assert.deepEqual(focusTarget(RECORDINGS, ch, null), { categoryId: "sp", index: null, lookUp: true });
  assert.deepEqual(focusTarget(ALL, ch, null), { categoryId: ALL, index: null, lookUp: true });
  assert.deepEqual(focusTarget("uk", { streamId: 7, categoryId: null }, null), { categoryId: ALL, index: null, lookUp: true });
  assert.deepEqual(listedFor(RECENT, [], [{ channel: { streamId: 4 } }]), [4]);
  assert.equal(listedFor("uk", [], []), null);
});
test("the fixed items lead in order, and say nothing until there is something to count", () => {
  const f = fixedRailItems({ favourites: 18, recent: 10, recordings: "1 now · 4 next", total: 49526 });
  assert.deepEqual(f.map((x) => x.id), [FAVOURITES, RECENT, RECORDINGS, ALL]);
  assert.deepEqual(f.map((x) => x.name), ["Favourites", "Recently watched", "Recordings", "All channels"]);
  assert.deepEqual(f.map((x) => x.count), ["18", "10", "1 now · 4 next", (49526).toLocaleString()]);
  const empty = fixedRailItems({ favourites: null, recent: 0, recordings: "", total: 0 });
  assert.deepEqual(empty.map((x) => x.count), ["", "", "", ""]);
});
