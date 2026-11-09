import test from "node:test";
import assert from "node:assert/strict";
import {
  BROWSER,
  MOCK_DESKTOP,
  SETTINGS_SECTIONS,
  allows,
  settingsSections,
} from "../logic/capabilities.ts";
import { ACTIONS, availableActions } from "../logic/palette.ts";

const ids = (actions) => actions.map((a) => a.id);

test("the app's window can do everything but sign out", () => {
  for (const [k, v] of Object.entries(MOCK_DESKTOP)) assert.equal(v, k !== "signOut", k);
  assert.deepEqual(settingsSections(MOCK_DESKTOP), [...SETTINGS_SECTIONS]);
});

test("Online services follow Data, in the app and in a browser", () => {
  for (const caps of [MOCK_DESKTOP, BROWSER]) {
    const s = settingsSections(caps);
    assert.equal(s[s.indexOf("Data") + 1], "Online services");
  }
});

test("Movies are on offer in the app and in a browser alike", () => {
  assert.equal(MOCK_DESKTOP.onDemand, true);
  assert.equal(BROWSER.onDemand, true);
});

test("a browser shows a movie's text subtitles, not those that are pictures", () => {
  assert.equal(MOCK_DESKTOP.pictureSubtitles, true);
  assert.equal(BROWSER.pictureSubtitles, false);
});

test("a browser keeps the guide, recordings, the account, refreshing and watching, and nothing of the computer's", () => {
  assert.deepEqual(settingsSections(BROWSER), [
    "Account",
    "Guide",
    "Recording",
    "Data",
    "Online services",
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
  const desktop = ids(availableActions(true, { ...everything, capabilities: MOCK_DESKTOP }));
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
