// Settings arithmetic: the clamps that stand between a stored config and mpv,
// which commands a change actually needs, and the subtitle preview — the one
// piece of this UI that stands in for something nobody can see, since libass
// draws the real subtitles into a plane beneath the window.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  DEFAULT_SETTINGS,
  DEFAULT_SUBTITLE,
  REWIND_MAX,
  REWIND_MIN,
  clampSettings,
  clampSubtitle,
  normaliseColor,
  planSave,
  previewFontPx,
  subtitlePreview,
} from "../logic/settings.ts";

const subtitle = (p) => ({ ...DEFAULT_SUBTITLE, ...p });
const settings = (p) => ({ ...DEFAULT_SETTINGS, ...p });

test("the defaults are the ones the contract documents", () => {
  // src/types.ts: backdropOpacity "0–1, defaults to 0.25", backdrop modelled
  // on Vimeo's controls with "background" as the ordinary case.
  assert.equal(DEFAULT_SETTINGS.subtitle.backdrop, "background");
  assert.equal(DEFAULT_SETTINGS.subtitle.backdropOpacity, 0.25);
  assert.equal(DEFAULT_SETTINGS.guideHours, 3);
  assert.ok(DEFAULT_SETTINGS.rewindMinutes >= REWIND_MIN);
  assert.ok(DEFAULT_SETTINGS.rewindMinutes <= REWIND_MAX);
});

test("rewind minutes are clamped to the slider's range, whole minutes only", () => {
  assert.equal(clampSettings(settings({ rewindMinutes: 0 })).rewindMinutes, 1);
  assert.equal(clampSettings(settings({ rewindMinutes: 900 })).rewindMinutes, 15);
  assert.equal(clampSettings(settings({ rewindMinutes: 7.4 })).rewindMinutes, 7);
  assert.equal(clampSettings(settings({ rewindMinutes: NaN })).rewindMinutes, 1);
});

test("guide hours and sync interval are bounded too", () => {
  assert.equal(clampSettings(settings({ guideHours: 99 })).guideHours, 6);
  assert.equal(clampSettings(settings({ guideHours: 0 })).guideHours, 1);
  assert.equal(clampSettings(settings({ syncIntervalHours: 100 })).syncIntervalHours, 24);
  assert.equal(clampSettings(settings({ syncIntervalHours: 0 })).syncIntervalHours, 1);
});

test("a colour that libass would choke on becomes white, not garbage", () => {
  assert.equal(normaliseColor("#FFAA00"), "#ffaa00");
  assert.equal(normaliseColor("#fa0"), "#ffaa00");
  assert.equal(normaliseColor("  #ffffff "), "#ffffff");
  assert.equal(normaliseColor("red"), "#ffffff");
  assert.equal(normaliseColor(""), "#ffffff");
});

test("subtitle numbers are clamped and the font never goes empty", () => {
  const s = clampSubtitle(
    subtitle({ size: 500, backdropOpacity: 9, outlineSize: -3, shadowOffset: 99, font: "  " }),
  );
  assert.equal(s.size, 72);
  assert.equal(s.backdropOpacity, 1);
  assert.equal(s.outlineSize, 0);
  assert.equal(s.shadowOffset, 8);
  assert.equal(s.font, "Sans");
});

// ------------------------------------------------------------- what to call

test("only a playback change costs applyPlaybackSettings", () => {
  const before = DEFAULT_SETTINGS;
  assert.deepEqual(planSave(before, settings({ rewindMinutes: 9 })), {
    applyPlayback: true,
    applySubtitle: false,
  });
  assert.deepEqual(planSave(before, settings({ hardwareDecode: false })), {
    applyPlayback: true,
    applySubtitle: false,
  });
});

test("only a subtitle change costs setSubtitleStyle", () => {
  const after = settings({ subtitle: subtitle({ size: 40 }) });
  assert.deepEqual(planSave(DEFAULT_SETTINGS, after), {
    applyPlayback: false,
    applySubtitle: true,
  });
});

test("the guide's own settings need neither: they are ours to keep", () => {
  assert.deepEqual(planSave(DEFAULT_SETTINGS, settings({ guideHours: 5 })), {
    applyPlayback: false,
    applySubtitle: false,
  });
  assert.deepEqual(planSave(DEFAULT_SETTINGS, settings({ syncIntervalHours: 8 })), {
    applyPlayback: false,
    applySubtitle: false,
  });
});

test("no change at all asks for nothing", () => {
  assert.deepEqual(planSave(DEFAULT_SETTINGS, settings({})), {
    applyPlayback: false,
    applySubtitle: false,
  });
});

// ----------------------------------------------------------------- preview

test("size is points at 720p, scaled to whatever the preview is", () => {
  assert.equal(previewFontPx(36, 720), 36);
  assert.equal(previewFontPx(36, 360), 18);
  assert.equal(previewFontPx(72, 144), 14.4);
  // Never small enough to vanish, whatever the frame.
  assert.ok(previewFontPx(12, 10) >= 6);
});

test("background paints behind the text, window paints a band", () => {
  const bg = subtitlePreview(subtitle({ backdrop: "background", backdropOpacity: 0.25 }), 150);
  assert.equal(bg.band, null);
  assert.equal(bg.text.background, "rgba(0, 0, 0, 0.25)");
  assert.notEqual(bg.text.padding, "0");

  const win = subtitlePreview(subtitle({ backdrop: "window", backdropOpacity: 0.6 }), 150);
  assert.deepEqual(win.band, { background: "rgba(0, 0, 0, 0.6)" });
  assert.equal(win.text.background, "transparent");
});

test("outline draws an edge all the way round, shadow only one way", () => {
  const out = subtitlePreview(subtitle({ backdrop: "outline", outlineSize: 2 }), 720);
  assert.equal(out.text.background, "transparent");
  assert.equal(out.text.textShadow.split(",").length, 8, "eight offsets");
  assert.ok(out.text.textShadow.includes("-2px -2px"));
  assert.ok(out.text.textShadow.includes("2px 2px"));

  const sh = subtitlePreview(subtitle({ backdrop: "shadow", shadowOffset: 3 }), 720);
  assert.ok(sh.text.textShadow.startsWith("3px 3px"), sh.text.textShadow);
  // One direction only: no offset going the other way.
  assert.ok(!sh.text.textShadow.includes("-3px"));
});

test("a zero outline or shadow paints nothing rather than a black smear", () => {
  assert.equal(
    subtitlePreview(subtitle({ backdrop: "outline", outlineSize: 0 }), 720).text.textShadow,
    "none",
  );
  assert.equal(
    subtitlePreview(subtitle({ backdrop: "shadow", shadowOffset: 0 }), 720).text.textShadow,
    "none",
  );
});

test("none is exactly nothing: no band, no box, no shadow", () => {
  const p = subtitlePreview(subtitle({ backdrop: "none" }), 150);
  assert.equal(p.band, null);
  assert.equal(p.text.background, "transparent");
  assert.equal(p.text.textShadow, "none");
});

test("the preview takes the chosen colour, normalised", () => {
  assert.equal(subtitlePreview(subtitle({ color: "#F2E600" }), 150).text.color, "#f2e600");
  assert.equal(subtitlePreview(subtitle({ color: "chartreuse" }), 150).text.color, "#ffffff");
});
