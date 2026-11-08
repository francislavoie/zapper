// The mouse's back and forward buttons: the Escape chain they share with the
// key (logic/escape.ts), the history of tabs and rail choices they walk once
// nothing is open, and the latch that makes one press act once however many
// events it arrives as (logic/navHistory.ts).

import test from "node:test";
import assert from "node:assert/strict";
import { escapeStep } from "../logic/escape.ts";
import {
  MOUSE_BACK,
  MOUSE_FORWARD,
  NAV_HISTORY_CAP,
  SAME_PRESS_MS,
  buttonLatch,
  isSideButton,
  goBack,
  goForward,
  startHistory,
  visit,
} from "../logic/navHistory.ts";

const nothing = {
  findSubtitles: false,
  programmeMenu: false,
  favouriteMenu: false,
  categoryMenu: false,
  categoryOptions: false,
  moviesMenu: false,
  showsMenu: false,
  continueMenu: false,
  videoMenu: false,
  menuOpen: false,
  aboutOpen: false,
  updateOpen: false,
  settingsOpen: false,
  paletteOpen: false,
  movieSheet: false,
  showSheet: false,
  showPane: false,
  upNextCard: false,
  fullscreen: false,
  query: false,
  queryFocused: false,
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

test("the subtitle finder over the player is closed alone, by either", () => {
  // Opened from the picture's menu, which has closed: the dialog is all
  // that is open over the full player.
  const c = open({ findSubtitles: true, mode: "watch", playing: true });
  assert.equal(escapeStep(c, "key"), "findSubtitles");
  assert.equal(escapeStep(c, "mouse"), "findSubtitles");
  // Over everything else too: it is the top-most thing there is.
  const all = open({ findSubtitles: true, videoMenu: true, settingsOpen: true, fullscreen: true, mode: "watch", playing: true });
  assert.equal(escapeStep(all, "key"), "findSubtitles");
  assert.equal(escapeStep(all, "mouse"), "findSubtitles");
  // Once it is closed, the next step is the player's, not before.
  assert.equal(escapeStep({ ...c, findSubtitles: false }, "mouse"), "browse");
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

test("Escape unwinds everything, one press each, in the order it was stacked", () => {
  // Everything open at once, over the full player in fullscreen with a
  // search typed: each press takes the top one off.
  const order = [
    ["findSubtitles", "findSubtitles"],
    ["programmeMenu", "programmeMenu"],
    ["favouriteMenu", "favouriteMenu"],
    ["categoryMenu", "categoryMenu"],
    ["categoryOptions", "categoryOptions"],
    ["moviesMenu", "moviesMenu"],
    ["showsMenu", "showsMenu"],
    ["continueMenu", "continueMenu"],
    ["videoMenu", "videoMenu"],
    ["menuOpen", "menu"],
    ["aboutOpen", "about"],
    ["updateOpen", "update"],
    ["settingsOpen", "settings"],
    ["paletteOpen", "palette"],
    ["movieSheet", "movieSheet"],
    ["showSheet", "showSheet"],
    ["upNextCard", "upNextCard"],
    ["showPane", "showPane"],
    ["fullscreen", "fullscreen"],
    ["query", "query"],
  ];
  const c = { ...nothing, mode: "watch", playing: true };
  for (const [flag] of order) c[flag] = true;
  for (const [flag, step] of order) {
    assert.equal(escapeStep(c, "key"), step, `with ${flag} on top`);
    c[flag] = false;
  }
  assert.equal(escapeStep(c, "key"), "browse");
  c.mode = "browse";
  assert.equal(escapeStep(c, "key"), "watch");
  c.playing = false;
  assert.equal(escapeStep(c, "key"), "none");
});

test("only a side button is a side button", () => {
  assert.equal(isSideButton({ button: MOUSE_BACK }), true);
  assert.equal(isSideButton({ button: MOUSE_FORWARD }), true);
  for (const button of [0, 1, 2]) assert.equal(isSideButton({ button }), false);
});

test("a step back over a tab the account no longer has goes past it", () => {
  let h = startHistory(live());
  h = visit(h, movies());
  h = visit(h, shows());
  const noMovies = (p) => p.section !== "movies";
  const b = goBack(h, noMovies);
  assert.deepEqual(b.place, live());
  assert.deepEqual(goForward(b.history, noMovies).place, shows());
  assert.equal(goBack(startHistory(movies()), noMovies), null);
});

test("the Shows pane: after the countdown, and a focused search is cleared first", () => {
  const c = { ...nothing, showPane: true, upNextCard: true, mode: "watch", playing: true };
  assert.equal(escapeStep(c, "key"), "upNextCard");
  assert.equal(escapeStep({ ...c, upNextCard: false }, "key"), "showPane");
  assert.equal(escapeStep({ ...c, upNextCard: false }, "mouse"), "showPane");
  const typed = { ...nothing, showPane: true, query: true, mode: "browse", playing: true };
  assert.equal(escapeStep(typed, "key"), "showPane", "a search not being typed in waits");
  assert.equal(escapeStep({ ...typed, queryFocused: true }, "key"), "query");
  assert.equal(escapeStep({ ...typed, queryFocused: true }, "mouse"), "showPane");
  // With something playing, Esc from the list closes the pane, not into the player.
  assert.equal(escapeStep({ ...nothing, showPane: true, playing: true }, "key"), "showPane");
});

test("Escape closes the Resume question before Up next's card, and the mouse leaves it", () => {
  const c = { ...nothing, mode: "watch", playing: true, resumePrompt: true, upNextCard: true };
  assert.equal(escapeStep(c, "key"), "resumePrompt");
  assert.equal(escapeStep({ ...c, paletteOpen: true }, "key"), "palette");
  assert.equal(escapeStep(c, "mouse"), "browse");
  assert.equal(escapeStep({ ...c, resumePrompt: false }, "key"), "upNextCard");
});
