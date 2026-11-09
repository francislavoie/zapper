// What the update badge, dialog and settings pane make of the updater's
// state. The decisions themselves (which release is newer, which asset this
// install takes, whether the download matches SHA256SUMS) are Rust's and are
// tested there: crates/zapper-core/src/updates/. These are the words and the next
// step, which is where the interface can be wrong on its own.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  announcement,
  asksForPassword,
  aurNote,
  badgeLabel,
  badgeShort,
  formatSize,
  kindLabel,
  nextStep,
  percentOf,
  recordingPrompt,
  releaseOf,
  showsBadge,
  statusLine,
  stepLabel,
} from "../logic/updates.ts";
import { ACTIONS, filterActions } from "../logic/palette.ts";
import { hhmm, setTimeFormat } from "../logic/format.ts";
import { DEFAULT_SETTINGS, clampSettings } from "../logic/settings.ts";

const release = (p = {}) => ({
  version: "0.2.0",
  name: "zapper 0.2.0",
  notes: "",
  url: "https://github.com/Someone/zapper/releases/tag/v0.2.0",
  prerelease: false,
  publishedAt: null,
  asset: { name: "zapper_0.2.0_amd64.deb", size: 9_400_000 },
  ...p,
});

const status = (state, p = {}) => ({ current: "0.1.0", kind: "deb", lastChecked: null, state, ...p });

test("the badge appears only when there is something to act on", () => {
  for (const s of ["notConfigured", "idle", "checking", "upToDate"]) {
    assert.equal(showsBadge({ state: s }), false, s);
  }
  assert.equal(showsBadge({ state: "failed", message: "x", release: null }), false, "a failed check is not news");
  for (const s of [
    { state: "available", release: release() },
    { state: "downloading", release: release(), received: 1, total: 2 },
    { state: "ready", release: release() },
    { state: "installed", release: release() },
    { state: "failed", message: "x", release: release() },
  ]) {
    assert.equal(showsBadge(s), true, s.state);
    assert.notEqual(badgeLabel(s), "", s.state);
    const short = badgeShort(s);
    assert.ok(short !== "" && short.length <= 10, `${s.state}: "${short}" is too long for the title bar`);
  }
  assert.equal(badgeShort({ state: "downloading", release: release(), received: 1, total: 4 }), "25%");
  assert.equal(badgeLabel({ state: "available", release: release() }), "Update to 0.2.0");
  assert.equal(badgeLabel({ state: "installed", release: release() }), "Restart to update");
});

test("download progress is a whole percent, never past 100, never NaN", () => {
  const d = (received, total) => ({ state: "downloading", release: release(), received, total });
  assert.equal(percentOf(d(0, 100)), 0);
  assert.equal(percentOf(d(499, 1000)), 49);
  assert.equal(percentOf(d(2000, 1000)), 100);
  assert.equal(percentOf(d(5, 0)), 0, "an unknown size");
  assert.equal(percentOf({ state: "ready", release: release() }), 0);
  assert.equal(badgeLabel(d(1, 4)), "Downloading 25%");
});

test("sizes read the way a person says them", () => {
  assert.equal(formatSize(9_400_000), "9.0 MB");
  assert.equal(formatSize(640 * 1024), "640 KB");
  assert.equal(formatSize(10), "1 KB", "never 0 KB for something that exists");
});

test("the main button always does the next sensible thing", () => {
  assert.equal(nextStep({ state: "notConfigured" }), null, "nothing to do, and the dialog says why");
  assert.equal(nextStep({ state: "idle" }), "check");
  assert.equal(nextStep({ state: "upToDate" }), "check");
  assert.equal(nextStep({ state: "checking" }), null);
  assert.equal(nextStep({ state: "available", release: release() }), "download");
  assert.equal(nextStep({ state: "available", release: release({ asset: null }) }), "page", "a build from source");
  assert.equal(nextStep({ state: "downloading", release: release(), received: 1, total: 2 }), "cancel");
  assert.equal(nextStep({ state: "ready", release: release() }), "install");
  assert.equal(nextStep({ state: "installing", release: release() }), null);
  // Waiting for a recording to finish can be called off.
  assert.equal(nextStep({ state: "waitingForRecording", release: release(), until: null }), "cancel");
  assert.equal(nextStep({ state: "installed", release: release() }), "restart");
  assert.equal(nextStep({ state: "failed", message: "x", release: release() }), "download");
  assert.equal(nextStep({ state: "failed", message: "x", release: release({ asset: null }) }), "page");
  assert.equal(nextStep({ state: "failed", message: "x", release: null }), "check");
  assert.equal(stepLabel("download", { state: "failed", message: "x", release: release() }), "Try again");
  assert.equal(stepLabel("download", { state: "available", release: release() }), "Download and install");
});

test("an update is announced once per version, and only when it turns up", () => {
  const avail = { state: "available", release: release() };
  assert.match(announcement(avail, null), /0\.2\.0 is available/);
  assert.equal(announcement(avail, "0.2.0"), null, "the daily check finding it again");
  assert.match(announcement({ state: "available", release: release({ version: "0.3.0" }) }, "0.2.0"), /0\.3\.0/);
  assert.equal(announcement({ state: "upToDate" }, null), null);
  assert.equal(announcement({ state: "ready", release: release() }, null), null);
});

test("a day of checks announces an update once, whatever states come between", () => {
  // What the hook does: remember the version once it has been announced.
  // The states between two daily checks include ones with no release at all
  // (a failed check, a Checking from before anything was offered), which is
  // exactly where comparing with the previous state announced it again.
  const avail = { state: "available", release: release() };
  const day = [
    { state: "checking" },
    avail,
    { state: "checking" },
    { state: "failed", message: "offline", release: null },
    avail,
    { state: "downloading", release: release(), received: 1, total: 2 },
    avail,
    { state: "available", release: release({ version: "0.3.0" }) },
    { state: "available", release: release({ version: "0.3.0" }) },
  ];
  let announced = null;
  const toasts = [];
  for (const s of day) {
    const text = announcement(s, announced);
    if (text) {
      announced = s.release.version;
      toasts.push(text);
    }
  }
  assert.equal(toasts.length, 2, toasts.join(" | "));
  assert.match(toasts[0], /0\.2\.0/);
  assert.match(toasts[1], /0\.3\.0/);
});

test("every state has a sentence, and not configured says why", () => {
  const states = [
    { state: "notConfigured" },
    { state: "idle" },
    { state: "checking" },
    { state: "upToDate" },
    { state: "available", release: release() },
    { state: "downloading", release: release(), received: 1, total: 2 },
    { state: "ready", release: release() },
    { state: "installing", release: release() },
    { state: "waitingForRecording", release: release(), until: null },
    { state: "installed", release: release() },
    { state: "failed", message: "The download doesn't match its checksum", release: release() },
  ];
  for (const s of states) assert.ok(statusLine(status(s)).length > 5, s.state);
  assert.match(statusLine(status({ state: "notConfigured" })), /aren't configured/);
  assert.equal(statusLine(status({ state: "upToDate" })), "zapper 0.1.0 is the newest version.");
  assert.match(statusLine(status(states.at(-1))), /checksum/, "a failure says what failed");
});

test("an install that would stop a recording says what, until when, and offers both ways", () => {
  setTimeFormat("24h");
  const until = Math.floor(new Date(2026, 10, 3, 21, 5).getTime() / 1000);
  const p = recordingPrompt(until, "The Late Film");
  assert.match(p.message, /recording “The Late Film” until 21:05/);
  assert.match(p.message, /what it has recorded so far is kept/);
  assert.equal(p.wait, "Install at 21:05, when it finishes");
  assert.equal(p.now, "Install now");
  // A service that cannot say: no title, no time, still two choices.
  const q = recordingPrompt(null, null);
  assert.match(q.message, /recording a programme\./);
  assert.equal(q.wait, "Install when it finishes");
  // The time follows the clock setting, like every other.
  setTimeFormat("12h");
  assert.equal(recordingPrompt(until, null).wait, `Install at ${hhmm(until)}, when it finishes`);
  assert.match(recordingPrompt(until, null).wait, /9:05 pm/);
  setTimeFormat("24h");
  // And the waiting state says when it will happen.
  const w = status({ state: "waitingForRecording", release: release(), until });
  assert.match(statusLine(w), /installs when the recording finishes, at 21:05\./);
  assert.equal(badgeShort(w.state), "Waiting");
  assert.ok(showsBadge(w.state));
});

test("install kinds have names, and the package managers warn about the password", () => {
  for (const k of ["deb", "rpm", "arch", "nsis", "msi", "dmg", "none"]) assert.ok(kindLabel(k).length > 0, k);
  assert.equal(asksForPassword("deb"), true);
  assert.equal(asksForPassword("rpm"), true);
  assert.equal(asksForPassword("arch"), true);
  assert.match(aurNote("arch"), /AUR helper/);
  assert.equal(aurNote("deb"), null);
  assert.equal(asksForPassword("dmg"), false);
  assert.equal(asksForPassword("none"), false);
});

test("releaseOf finds the release wherever a state carries one", () => {
  assert.equal(releaseOf({ state: "idle" }), null);
  assert.equal(releaseOf({ state: "ready", release: release() }).version, "0.2.0");
  assert.equal(releaseOf({ state: "failed", message: "x", release: null }), null);
});

test("the palette finds the update action by the words people use", () => {
  for (const q of ["update", "upgrade", "version", "check"]) {
    assert.ok(
      filterActions(ACTIONS, q).some((a) => a.id === "updates"),
      `"${q}" finds Check for updates`,
    );
  }
});

test("the automatic check is on unless someone turned it off", () => {
  assert.equal(DEFAULT_SETTINGS.checkForUpdates, true);
  assert.equal(clampSettings({ ...DEFAULT_SETTINGS, checkForUpdates: false }).checkForUpdates, false);
  // A stored config from before the setting existed.
  const old = { ...DEFAULT_SETTINGS };
  delete old.checkForUpdates;
  assert.equal(clampSettings(old).checkForUpdates, true);
});
