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

test("every open applies the standing choice", () => {
  const list = [track(1, { lang: "eng" }), track(2, { lang: "fra" })];
  assert.equal(openChoice(on("fra"), list, true), 2);
  assert.equal(openChoice(on("deu"), list, true), 1);
  assert.equal(openChoice(off("fra"), list, true), null);
  // No subtitles: nothing to do.
  assert.equal(openChoice(on("fra"), [], true), undefined);
  assert.equal(openChoice(off(), [], true), undefined);
  // Pictures where pictures cannot be shown: as good as none.
  const pictures = [track(1, { lang: "fra", codec: "dvb_subtitle" })];
  assert.equal(openChoice(on("fra"), pictures, false), undefined);
  assert.equal(openChoice(on("fra"), pictures, true), 1);
});

test("the toggle turns shown subtitles off, and hidden ones on", () => {
  const list = [track(1, { lang: "eng" }), track(2, { lang: "fra" })];
  assert.equal(toggleChoice(list, "fra", true), 2);
  assert.equal(toggleChoice(list, null, true), 1);
  const showing = [track(1, { lang: "eng" }), track(2, { lang: "fra", selected: true })];
  assert.equal(toggleChoice(showing, "fra", true), null);
  assert.equal(toggleChoice([], "fra", true), undefined);
  assert.equal(toggleChoice([track(1, { codec: "hdmv_pgs_subtitle" })], null, false), undefined);
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
  assert.equal(subtitlesShowing([track(1, {})]), false);
  assert.equal(subtitlesShowing([track(1, {}), track(2, { selected: true })]), true);
  assert.match(subtitleTip(false), /^Subtitles \(C\)$/);
  assert.match(subtitleTip(true), /^Subtitles off \(C\)$/);
});
