// The mouse's back and forward buttons: the Escape chain they share with the
// key (logic/escape.ts), the history of tabs and rail choices they walk once
// nothing is open, and the latch that makes one press act once however many
// events it arrives as (logic/navHistory.ts).

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { escapeStep } from "../logic/escape.ts";
import {
  MOUSE_BACK,
  MOUSE_FORWARD,
  NAV_HISTORY_CAP,
  SAME_PRESS_MS,
  buttonLatch,
  goBack,
  goForward,
  startHistory,
  visit,
} from "../logic/navHistory.ts";

const nothing = {
  programmeMenu: false,
  favouriteMenu: false,
  categoryMenu: false,
  categoryOptions: false,
  moviesMenu: false,
  showsMenu: false,
  videoMenu: false,
  menuOpen: false,
  aboutOpen: false,
  updateOpen: false,
  settingsOpen: false,
  paletteOpen: false,
  movieSheet: false,
  showSheet: false,
  upNextCard: false,
  fullscreen: false,
  query: false,
  mode: "browse",
  playing: false,
};
const open = (over) => ({ ...nothing, ...over });

test("a menu goes before the sheet under it, and the sheet before the player", () => {
  const c = open({ showsMenu: true, showSheet: true, mode: "browse", playing: true });
  assert.equal(escapeStep(c, "mouse"), "showsMenu");
  assert.equal(escapeStep({ ...c, showsMenu: false }, "mouse"), "showSheet");
  assert.equal(escapeStep(open({ mode: "watch", playing: true }), "mouse"), "browse");
});

test("with nothing open, back goes through the history; Escape goes to the picture", () => {
  const c = open({ playing: true });
  assert.equal(escapeStep(c, "mouse"), "history");
  assert.equal(escapeStep(c, "key"), "watch");
  assert.equal(escapeStep(nothing, "key"), "none");
});

test("back leaves Up next and the search alone; Escape takes them", () => {
  const card = open({ upNextCard: true, mode: "watch", playing: true });
  assert.equal(escapeStep(card, "key"), "upNextCard");
  assert.equal(escapeStep(card, "mouse"), "browse");
  const search = open({ query: true });
  assert.equal(escapeStep(search, "key"), "query");
  assert.equal(escapeStep(search, "mouse"), "history");
});

test("fullscreen is left before anything below it, by either", () => {
  const c = open({ fullscreen: true, mode: "watch", playing: true });
  assert.equal(escapeStep(c, "key"), "fullscreen");
  assert.equal(escapeStep(c, "mouse"), "fullscreen");
});

test("the guide goes back to the list before any history", () => {
  assert.equal(escapeStep(open({ mode: "guide" }), "mouse"), "browse");
});

const live = (rail = "all") => ({ section: "live", rail });
const movies = (rail = "recent") => ({ section: "movies", rail });
const shows = (rail = "recent") => ({ section: "shows", rail });

test("back and forward walk the places visited", () => {
  let h = startHistory(live());
  h = visit(h, movies());
  h = visit(h, shows());
  const b1 = goBack(h);
  assert.deepEqual(b1.place, movies());
  // Arriving there is not a new step.
  assert.equal(visit(b1.history, movies()), b1.history);
  const b2 = goBack(b1.history);
  assert.deepEqual(b2.place, live());
  assert.equal(goBack(b2.history), null);
  const f = goForward(b2.history);
  assert.deepEqual(f.place, movies());
  assert.deepEqual(goForward(f.history).place, shows());
  assert.equal(goForward(goForward(f.history).history), null);
});

test("a rail choice within a tab is a step", () => {
  let h = startHistory(live());
  h = visit(h, live("fav"));
  assert.deepEqual(goBack(h).place, live());
});

test("a new step after a back drops what forward would have gone to", () => {
  let h = startHistory(live());
  h = visit(h, movies());
  h = goBack(h).history;
  h = visit(h, shows());
  assert.equal(goForward(h), null);
  assert.deepEqual(goBack(h).place, live());
});

test("the history keeps the last few places, not the session", () => {
  let h = startHistory(live("0"));
  for (let i = 1; i <= NAV_HISTORY_CAP + 10; i++) h = visit(h, live(String(i)));
  assert.equal(h.places.length, NAV_HISTORY_CAP);
  assert.deepEqual(h.places[h.at], live(String(NAV_HISTORY_CAP + 10)));
  let steps = 0;
  for (let b = goBack(h); b; b = goBack(b.history)) steps++;
  assert.equal(steps, NAV_HISTORY_CAP - 1);
});

test("one press acts once, whichever events it arrives as", () => {
  const latch = buttonLatch();
  // Chromium: pointerdown, mousedown, pointerup, mouseup, auxclick.
  assert.equal(latch("down", MOUSE_BACK, 0), false);
  assert.equal(latch("down", MOUSE_BACK, 0), false);
  assert.equal(latch("up", MOUSE_BACK, 80), true);
  assert.equal(latch("up", MOUSE_BACK, 80), false);
  assert.equal(latch("up", MOUSE_BACK, 81), false);
  // The next press acts again, however soon.
  latch("down", MOUSE_BACK, 150);
  assert.equal(latch("up", MOUSE_BACK, 200), true);
});

test("an engine that sends no press still acts, once per release", () => {
  const latch = buttonLatch();
  assert.equal(latch("up", MOUSE_BACK, 1000), true);
  assert.equal(latch("up", MOUSE_BACK, 1001), false); // the same release, as auxclick
  assert.equal(latch("up", MOUSE_BACK, 1000 + SAME_PRESS_MS + 1), true);
  assert.equal(latch("up", MOUSE_FORWARD, 1000 + SAME_PRESS_MS + 2), true);
});

test("useGlobalKeys runs both Escape and the side buttons through the one chain", () => {
  const src = readFileSync(new URL("../hooks/useGlobalKeys.ts", import.meta.url), "utf8");
  assert.match(src, /unwind\("key"\)/);
  assert.match(src, /unwind\("mouse"\)/);
  // No second list: the chain's steps are dispatched from one switch.
  assert.equal((src.match(/escapeStep\(/g) ?? []).length, 2);
  assert.match(src, /e\.preventDefault\(\);\s*const down/);
});
