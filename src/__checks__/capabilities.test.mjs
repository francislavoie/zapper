import test from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import {
  BROWSER,
  DESKTOP,
  SETTINGS_SECTIONS,
  allows,
  settingsSections,
} from "../logic/capabilities.ts";
import { ACTIONS, availableActions } from "../logic/palette.ts";

const ids = (actions) => actions.map((a) => a.id);

test("the app's window can do everything but sign out", () => {
  for (const [k, v] of Object.entries(DESKTOP)) assert.equal(v, k !== "signOut", k);
  assert.deepEqual(settingsSections(DESKTOP), [...SETTINGS_SECTIONS]);
});

test("Movies are on offer in the app and in a browser alike", () => {
  assert.equal(DESKTOP.onDemand, true);
  assert.equal(BROWSER.onDemand, true);
});

test("a browser shows a movie's text subtitles, not those that are pictures", () => {
  assert.equal(DESKTOP.pictureSubtitles, true);
  assert.equal(BROWSER.pictureSubtitles, false);
});

test("a browser keeps the guide, recordings, the account, refreshing and watching, and nothing of the computer's", () => {
  assert.deepEqual(settingsSections(BROWSER), [
    "Account",
    "Guide",
    "Recording",
    "Data",
    "Keyboard",
    "About",
  ]);
  for (const k of [
    "catchup",
    "mpv",
    "window",
    "mediaKeys",
    "updater",
    "data",
    "localService",
    "networkAdmin",
  ]) {
    assert.equal(BROWSER[k], false, k);
  }
  // The service keeps the account and the catalogue, so a browser signed in
  // to it may change the one and refresh the other.
  assert.equal(BROWSER.account, true);
  // Watching goes through the service's own stream (src/webPlayer.ts).
  assert.equal(BROWSER.playback, true);
  assert.equal(BROWSER.resync, true);
  assert.equal(BROWSER.signOut, true);
  assert.equal(allows(BROWSER, undefined), true);
  assert.equal(allows(BROWSER, "window"), false);
});

test("the menu and the palette offer only what can be done here", () => {
  const everything = { hasPrevious: true, sleeping: true, programme: true, recordable: true };
  const desktop = ids(availableActions(true, { ...everything, capabilities: DESKTOP }));
  assert.ok(desktop.includes("quit") && desktop.includes("updates") && desktop.includes("pause"));
  assert.ok(!desktop.includes("signOut"), "nothing to sign out of in the app");
  // The default is the app's window, so callers that say nothing are unchanged.
  assert.deepEqual(ids(availableActions(true, everything)), desktop);
  assert.equal(desktop.length, ACTIONS.length - 1);

  // In a browser the player's controls are there; mpv's statistics and the
  // window's are not.
  const browser = ids(availableActions(true, { ...everything, capabilities: BROWSER }));
  for (const gone of ["quit", "updates", "stats"]) {
    assert.ok(!browser.includes(gone), gone);
  }
  for (const kept of [
    "browse",
    "guide",
    "sync",
    "recordings",
    "settings",
    "about",
    "signOut",
    "previous",
    "pause",
    "stop",
    "live",
    "sleep30",
    "record",
  ]) {
    assert.ok(browser.includes(kept), kept);
  }
  // Where it can't play, no player control either.
  const cannot = ids(
    availableActions(true, { ...everything, capabilities: { ...BROWSER, playback: false } }),
  );
  for (const gone of ["previous", "pause", "stop", "live", "sleep30"]) {
    assert.ok(!cannot.includes(gone), gone);
  }
});

// What the interface can do is asked of `capabilities`, not worked out from
// where it is running: only the switch itself and the modules that wrap
// Tauri's own window calls (the window, fullscreen, page zoom) know about
// Tauri.
test("nothing outside the backend switch asks whether it is in Tauri", () => {
  const allowed = new Set(["backend.ts", "window.ts", "fullscreen.ts", "zoom.ts"]);
  const root = new URL("../", import.meta.url);
  const walk = (dir) =>
    readdirSync(dir).flatMap((name) => {
      const path = new URL(name, dir);
      if (statSync(path).isDirectory()) return name === "__checks__" ? [] : walk(new URL(`${name}/`, dir));
      return /\.(ts|tsx)$/.test(name) ? [[path, path.pathname.slice(root.pathname.length)]] : [];
    });
  const offenders = walk(root)
    .filter(([, rel]) => !allowed.has(rel))
    .filter(([path]) => /\binTauri\b|__TAURI/.test(readFileSync(path, "utf8")))
    .map(([, rel]) => rel);
  assert.deepEqual(offenders, []);
});
