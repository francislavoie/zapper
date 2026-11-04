import test from "node:test";
import assert from "node:assert/strict";
import {
  TOOLBAR_HEIGHT,
  isBusy,
  maximiseLabel,
  relativeTime,
  restingText,
  statusLine,
  NO_ACCOUNT_MESSAGE,
  SECTION_LABEL,
  titleTabs,
} from "../logic/toolbar.ts";
import { readFileSync } from "node:fs";
import {
  ACTIONS,
  ACTION_IDS,
  GROUP_ORDER,
  actionGroups,
  availableActions,
  flatGroups,
} from "../logic/palette.ts";

const sync = (over = {}) => ({
  phase: "idle",
  message: "",
  channelsTotal: 0,
  lastSynced: null,
  ...over,
});

const MIN = 60;
const HOUR = 3600;
const DAY = 86400;

// ------------------------------------------------------------ status text

test("each working phase gets its own sentence, in this app's words", () => {
  assert.equal(statusLine(sync({ phase: "categories" }), 0).text, "Refreshing categories…");
  assert.equal(statusLine(sync({ phase: "channels" }), 0).text, "Refreshing channels…");
  // "epg" is the wire word. The user-facing word is "guide", everywhere.
  assert.equal(statusLine(sync({ phase: "epg" }), 0).text, "Loading guide…");
  assert.equal(statusLine(sync({ phase: "movies" }), 0).text, "Refreshing movies…");
});

test("the spinner turns for exactly the phases that are work", () => {
  for (const phase of ["categories", "channels", "epg", "movies"]) {
    assert.ok(isBusy(phase), phase);
    assert.equal(statusLine(sync({ phase }), 0).spinning, true);
    assert.equal(statusLine(sync({ phase }), 0).tone, "busy");
  }
  for (const phase of ["idle", "failed"]) {
    assert.ok(!isBusy(phase), phase);
    assert.equal(statusLine(sync({ phase }), 0).spinning, false);
  }
});

test("the backend's message rides along while working, and is not the headline", () => {
  const s = statusLine(sync({ phase: "channels", message: "60,000 channels" }), 0);
  assert.equal(s.text, "Refreshing channels…");
  assert.equal(s.detail, "60,000 channels");
});

test("a failure promotes the backend's reason and offers no false calm", () => {
  const s = statusLine(sync({ phase: "failed", message: "server refused: 403" }), 0);
  assert.equal(s.tone, "bad");
  assert.equal(s.text, "server refused: 403");
  // A failure with nothing to say still says something.
  assert.equal(statusLine(sync({ phase: "failed" }), 0).text, "Sync failed");
});

test("a sync that went wrong offers to try again", () => {
  assert.equal(statusLine(sync({ phase: "failed", message: "server refused: 403" }), 0).action, "retry");
  assert.equal(statusLine(sync({ phase: "failed" }), 0).action, "retry");
});

test("no account offers to add one, not to retry", () => {
  // The backend's message, as the sync publishes it. Retrying cannot help:
  // nothing changes until there is an account to sync from.
  const s = statusLine(
    sync({ phase: "failed", message: "no account configured. Set one in Settings → Account." }),
    0,
  );
  assert.equal(s.action, "account");
  assert.equal(s.tone, "bad");
  // Not the backend's sentence, which points at a menu the link now opens.
  assert.ok(!/settings/i.test(s.text), s.text);
});

test("the no-account test matches what the backend actually says", () => {
  // Tied to the text rather than trusted: rewording the Rust error without
  // this would quietly bring "retry" back on a first run.
  const rs = readFileSync(
    new URL("../../src-tauri/src/catalog/credentials.rs", import.meta.url),
    "utf8",
  );
  const said = rs.match(/fn load_account\(\)[\s\S]*?anyhow!\("([^"]+)"/);
  assert.ok(said, "load_account's error was not found");
  assert.ok(said[1].toLowerCase().startsWith(NO_ACCOUNT_MESSAGE), said[1]);
});

test("nothing wrong, nothing to click", () => {
  for (const phase of ["idle", "categories", "channels", "epg"]) {
    assert.equal(statusLine(sync({ phase }), 0).action, null, phase);
  }
});

test("the resting line is never blank — it is the bar's whole content when idle", () => {
  const never = statusLine(sync({ phase: "idle" }), 1000);
  assert.equal(never.tone, "idle");
  assert.equal(never.text, "Not synced yet");

  const fresh = statusLine(sync({ phase: "idle", lastSynced: 1000 - 3 * HOUR }), 1000 * 1);
  assert.ok(fresh.text.startsWith("Synced "));
});

test("at rest the status says only when it synced, not the catalogue's size", () => {
  assert.equal(statusLine(sync({ channelsTotal: 0 }), 0).detail, "");
  assert.equal(statusLine(sync({ channelsTotal: 60000 }), 0).detail, "");
});

// ---------------------------------------------------------- relative time

test("relative time uses the largest whole unit that fits", () => {
  const now = 10 * DAY;
  assert.equal(relativeTime(now - 30, now), "just now");
  assert.equal(relativeTime(now - 59, now), "just now");
  assert.equal(relativeTime(now - MIN, now), "1 minute ago");
  assert.equal(relativeTime(now - 59 * MIN, now), "59 minutes ago");
  assert.equal(relativeTime(now - HOUR, now), "1 hour ago");
  assert.equal(relativeTime(now - 23 * HOUR, now), "23 hours ago");
  assert.equal(relativeTime(now - DAY, now), "1 day ago");
  assert.equal(relativeTime(now - 6 * DAY, now), "6 days ago");
  assert.equal(relativeTime(now - 7 * DAY, now), "1 week ago");
});

test("a clock that has gone backwards does not produce a negative age", () => {
  // Suspend, resync, a backend a few seconds ahead: none of these are worth
  // "-3 minutes ago" in permanent chrome.
  assert.equal(relativeTime(1000, 900), "just now");
  assert.equal(restingText(1000, 900), "Synced just now");
});

test("never synced is said, not implied by an empty line", () => {
  assert.equal(restingText(null, 12345), "Not synced yet");
});

// --------------------------------------------------- the menu is the palette

test("the menu's groups are a partition of the palette's actions", () => {
  for (const playing of [false, true]) {
    const available = availableActions(playing);
    const flat = flatGroups(actionGroups(available));
    assert.equal(flat.length, available.length);
    assert.deepEqual(new Set(flat.map((a) => a.id)), new Set(available.map((a) => a.id)));
  }
});

test("the menu can never offer an action the palette would not", () => {
  const menuIds = flatGroups(actionGroups(availableActions(false))).map((a) => a.id);
  assert.ok(!menuIds.includes("pause"), "player actions stay hidden with nothing playing");
  assert.ok(menuIds.includes("guide"));
  assert.ok(menuIds.includes("browse"));
  assert.ok(menuIds.includes("settings"));
  assert.ok(menuIds.includes("sync"));
  assert.ok(menuIds.includes("about"));
});

test("empty groups are dropped rather than drawn as a stray rule", () => {
  const idle = actionGroups(availableActions(false));
  assert.ok(idle.every((g) => g.length > 0));
  // With nothing playing the whole player group goes, and the sleep timer
  // with it — there is nothing for it to stop — so there are two fewer.
  assert.equal(actionGroups(availableActions(true)).length, idle.length + 2);
});

test("groups draw in a declared order, not in declaration order", () => {
  const groups = actionGroups(availableActions(true));
  const order = groups.map((g) => g[0].group);
  assert.deepEqual(order, GROUP_ORDER);
});

test("every action has a group, an id from the one list, and no duplicates", () => {
  const ids = ACTIONS.map((a) => a.id);
  assert.equal(new Set(ids).size, ids.length);
  assert.deepEqual(new Set(ids), new Set(ACTION_IDS));
  assert.ok(ACTIONS.every((a) => GROUP_ORDER.includes(a.group)));
});

// -------------------------------------------------------------- the window

test("the maximise button says which way it will go", () => {
  assert.equal(maximiseLabel(false).label, "Maximise");
  assert.equal(maximiseLabel(true).label, "Restore");
  assert.notEqual(maximiseLabel(true).glyph, maximiseLabel(false).glyph);
});

test("the toolbar is tall enough for its controls and their edge inset", () => {
  // 26px buttons, inset 6px from the window's resize edge at the top.
  assert.ok(TOOLBAR_HEIGHT >= 32, "a 26px control plus a 6px inset has to fit");
  assert.ok(TOOLBAR_HEIGHT <= 48, "a title bar this app drew should not be a header");
});

test("the title bar's tabs: Live TV always, Movies with on-demand and movies to show", () => {
  assert.deepEqual(titleTabs({ onDemand: false }), ["live"]);
  assert.deepEqual(titleTabs({ onDemand: true }), ["live", "movies"]);
  assert.deepEqual(titleTabs({ onDemand: true }).map((s) => SECTION_LABEL[s]), ["Live TV", "Movies"]);
  // An account without movies has no tab for them, whatever the app can do.
  assert.deepEqual(titleTabs({ onDemand: true }, false), ["live"]);
});
