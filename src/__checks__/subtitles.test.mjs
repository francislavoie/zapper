// The subtitles toggle and the standing choice behind it: which track turning
// them on shows, what a choice in the menu leaves the settings saying, and
// when the button is there at all.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  afterChoosing,
  hasSubtitles,
  openChoice,
  preferredIndex,
  shownTrack,
  subtitleTip,
  subtitlesShowing,
  toggleChoice,
} from "../logic/subtitles.ts";
import { DEFAULT_SETTINGS, clampSettings, planSave } from "../logic/settings.ts";

const track = (id, t) => ({
  id,
  title: null,
  lang: null,
  codec: "subrip",
  channels: null,
  default: false,
  forced: false,
  hearingImpaired: false,
  visualImpaired: false,
  selected: false,
  ...t,
});

const on = (subtitleLanguage) => ({ subtitlesOn: true, subtitleLanguage });
const off = (subtitleLanguage = null) => ({ subtitlesOn: false, subtitleLanguage });

test("subtitles start off, in no language, and the choice is kept and applied", () => {
  assert.equal(DEFAULT_SETTINGS.subtitlesOn, false);
  assert.equal(DEFAULT_SETTINGS.subtitleLanguage, null);
  // A config from before the setting reads as off.
  const older = { ...DEFAULT_SETTINGS };
  delete older.subtitlesOn;
  delete older.subtitleLanguage;
  const clean = clampSettings(older);
  assert.equal(clean.subtitlesOn, false);
  assert.equal(clean.subtitleLanguage, null);
  assert.equal(clampSettings({ ...DEFAULT_SETTINGS, subtitleLanguage: "" }).subtitleLanguage, null);
  assert.equal(clampSettings({ ...DEFAULT_SETTINGS, subtitleLanguage: 7 }).subtitleLanguage, null);
  assert.equal(clampSettings({ ...DEFAULT_SETTINGS, subtitleLanguage: "fra" }).subtitleLanguage, "fra");
  // The player is told: every open reads it.
  assert.equal(planSave(DEFAULT_SETTINGS, { ...DEFAULT_SETTINGS, subtitlesOn: true }).applyPlayback, true);
  assert.equal(
    planSave(DEFAULT_SETTINGS, { ...DEFAULT_SETTINGS, subtitleLanguage: "fra" }).applyPlayback,
    true,
  );
});

test("turning subtitles on shows the language last chosen, else the first", () => {
  const list = [track(1, { lang: "eng" }), track(2, { lang: "fra" })];
  assert.equal(preferredIndex(list, "fra"), 1);
  assert.equal(preferredIndex(list, "FRA"), 1);
  assert.equal(preferredIndex(list, "deu"), 0);
  assert.equal(preferredIndex(list, null), 0);
  assert.equal(preferredIndex([], "fra"), null);
  // Forced subtitles (signs, a line of another language) are passed over
  // when there is a full track to show.
  const forced = [track(1, { lang: "eng", forced: true }), track(2, { lang: "eng" }), track(3, { lang: "fra" })];
  assert.equal(preferredIndex(forced, "eng"), 1);
  assert.equal(preferredIndex(forced, "deu"), 1);
  assert.equal(preferredIndex([track(1, { lang: "eng", forced: true })], "eng"), 0);
  // Only what can be shown here.
  const pictures = [track(1, { lang: "fra", codec: "picture" }), track(2, { lang: "eng" })];
  assert.equal(preferredIndex(pictures, "fra", (t) => t.codec !== "picture"), 1);
  assert.equal(preferredIndex([pictures[0]], "fra", (t) => t.codec !== "picture"), null);
});

test("every open chooses a track, shown when subtitles are on and hidden when off", () => {
  const list = [track(1, { lang: "eng" }), track(2, { lang: "fra" })];
  assert.deepEqual(openChoice(on("fra"), list, true), { id: 2, visible: true });
  assert.deepEqual(openChoice(on("deu"), list, true), { id: 1, visible: true });
  // Off: the track all the same, hidden, so on is never a switch of tracks.
  assert.deepEqual(openChoice(off("fra"), list, true), { id: 2, visible: false });
  // No subtitles: nothing to do.
  assert.equal(openChoice(on("fra"), [], true), undefined);
  assert.equal(openChoice(off(), [], true), undefined);
  // Pictures where pictures cannot be shown: as good as none.
  const pictures = [track(1, { lang: "fra", codec: "dvb_subtitle" })];
  assert.equal(openChoice(on("fra"), pictures, false), undefined);
  assert.deepEqual(openChoice(on("fra"), pictures, true), { id: 1, visible: true });
});

test("the toggle shows and hides the track chosen, and switches only when there is none", () => {
  const chosen = [track(1, { lang: "eng" }), track(2, { lang: "fra", selected: true })];
  // Shown: hidden (null is Off, which keeps the track).
  assert.equal(toggleChoice(chosen, true, "eng", true), null);
  // Hidden: the same track shown again, whatever the language now wanted.
  assert.equal(toggleChoice(chosen, false, "eng", true), 2);
  // None chosen (an open from before, say): the one an open would choose.
  const none = [track(1, { lang: "eng" }), track(2, { lang: "fra" })];
  assert.equal(toggleChoice(none, false, "fra", true), 2);
  assert.equal(toggleChoice(none, true, null, true), 1);
  assert.equal(toggleChoice([], false, "fra", true), undefined);
  assert.equal(toggleChoice([track(1, { codec: "hdmv_pgs_subtitle" })], false, null, false), undefined);
});

test("the menu shows the track only while it is visible", () => {
  const chosen = [track(1, { lang: "eng" }), track(2, { lang: "fra", selected: true })];
  assert.equal(shownTrack({ subtitles: chosen, subtitlesVisible: true })?.id, 2);
  // Off in the menu is a hidden track.
  assert.equal(shownTrack({ subtitles: chosen, subtitlesVisible: false }), null);
  assert.equal(shownTrack({ subtitles: [track(1, {})], subtitlesVisible: true }), null);
});

test("choosing in the menu, or with the toggle, is the new standing choice", () => {
  assert.deepEqual(afterChoosing(track(2, { lang: "fra" }), off("eng")), on("fra"));
  // A track that names no language keeps the language there was.
  assert.deepEqual(afterChoosing(track(3, { lang: null }), off("eng")), on("eng"));
  assert.deepEqual(afterChoosing(track(3, { lang: "" }), off(null)), on(null));
  // Off keeps the language, for the next time they are turned on.
  assert.deepEqual(afterChoosing(null, on("fra")), off("fra"));
});

test("the button is there only with subtitles to show, lit while they show", () => {
  assert.equal(hasSubtitles([], true), false);
  assert.equal(hasSubtitles([track(1, { lang: "eng" })], true), true);
  assert.equal(hasSubtitles([track(1, { codec: "dvb_subtitle" })], false), false);
  assert.equal(hasSubtitles([track(1, { codec: "dvb_subtitle" })], true), true);
  const chosen = [track(1, {}), track(2, { selected: true })];
  assert.equal(subtitlesShowing({ subtitles: [track(1, {})], subtitlesVisible: true }), false);
  assert.equal(subtitlesShowing({ subtitles: chosen, subtitlesVisible: true }), true);
  assert.equal(subtitlesShowing({ subtitles: chosen, subtitlesVisible: false }), false);
  assert.match(subtitleTip(false), /^Subtitles \(C\)$/);
  assert.match(subtitleTip(true), /^Subtitles off \(C\)$/);
});
