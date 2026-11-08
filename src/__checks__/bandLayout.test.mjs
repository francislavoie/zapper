// The band across the top of browse mode: what it tells of in its main column
// and beside it, and how ↩ Show swaps the two until the next selection.
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { bandLayout, bandLeadAfter } from "../logic/band.ts";

const channel = { kind: "channel", key: "channel:1" };
const movie = { kind: "movie", key: "movie:7" };
const otherMovie = { kind: "movie", key: "movie:8" };
const show = { kind: "show", key: "show:3" };
const episode = { kind: "episode", key: "show:3" };
const base = { playing: null, selected: null, sheetOpen: false, lead: "selection" };

test("a selection while something else plays: both, the selection first", () => {
  assert.deepEqual(bandLayout({ ...base, playing: channel, selected: movie }), { main: "selection", side: "playing" });
  assert.deepEqual(bandLayout({ ...base, playing: episode, selected: otherMovie }), { main: "selection", side: "playing" });
});

test("↩ Show puts what plays first and the selection beside it", () => {
  assert.deepEqual(bandLayout({ ...base, playing: channel, selected: movie, lead: "playing" }), {
    main: "playing",
    side: "selection",
  });
});

test("one thing to tell of: one section, with context beside it", () => {
  assert.deepEqual(bandLayout({ ...base, playing: channel }), { main: "playing", side: "context" });
  assert.deepEqual(bandLayout({ ...base, selected: show }), { main: "selection", side: "context" });
  // The lead only matters while there are two.
  assert.deepEqual(bandLayout({ ...base, selected: movie, lead: "playing" }), { main: "selection", side: "context" });
  assert.deepEqual(bandLayout(base), { main: null, side: null });
});

test("the selection is what plays: one section, about what plays", () => {
  assert.deepEqual(bandLayout({ ...base, playing: episode, selected: show }), { main: "playing", side: "context" });
  assert.deepEqual(bandLayout({ ...base, playing: movie, selected: movie, lead: "playing" }), {
    main: "playing",
    side: "context",
  });
});

test("with a sheet open the band shows what plays, not a copy of the sheet", () => {
  assert.deepEqual(bandLayout({ ...base, playing: channel, selected: movie, sheetOpen: true }), {
    main: "playing",
    side: "context",
  });
  // With nothing playing it keeps the selection rather than going blank.
  assert.deepEqual(bandLayout({ ...base, selected: movie, sheetOpen: true }), { main: "selection", side: "context" });
});

test("↩ Show swaps, and lasts until the next selection", () => {
  const swap = { type: "bandSwap" };
  let lead = "selection";
  lead = bandLeadAfter(lead, swap);
  assert.equal(lead, "playing");
  // Anything else leaves it: a sheet, a menu, a mark, the player.
  for (const type of ["movies/sheet", "shows/pane", "shows/marked", "mode", "player", "section"]) {
    assert.equal(bandLeadAfter(lead, { type }), "playing", type);
  }
  assert.equal(bandLeadAfter(lead, swap), "selection");
  assert.equal(bandLeadAfter("playing", { type: "movies/selectMovie", movie: null }), "selection");
  assert.equal(bandLeadAfter("playing", { type: "shows/selectShow", show: null }), "selection");
});

test("the store runs every action through it, after its own reducer", () => {
  // store.tsx is TSX, so it is read rather than imported.
  const store = readFileSync(new URL("../store.tsx", import.meta.url), "utf8");
  const at = store.indexOf("export function reducer(");
  assert.ok(at >= 0);
  const body = store.slice(at, store.indexOf("\n}\n", at));
  assert.match(body, /bandLeadAfter\(state\.bandLead, action\)/);
  assert.match(body, /const next = reduce\(state, action\)/);
  assert.equal(body.match(/return /g)?.length, 1, "one way out, through the lead");
  assert.ok(!/nextBandLead|bandLead:/.test(store.slice(store.indexOf("function reduce("))), "no other place sets it");
});
