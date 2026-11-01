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
  REWIND_BITRATES,
  REWIND_MAX,
  REWIND_MIN,
  clampSettings,
  rewindSeconds,
  clampSubtitle,
  normaliseColor,
  planSave,
  previewFontPx,
  subtitlePreview,
  isDefault,
  resetPatch,
} from "../logic/settings.ts";

const subtitle = (p) => ({ ...DEFAULT_SUBTITLE, ...p });
const settings = (p) => ({ ...DEFAULT_SETTINGS, ...p });

test("the defaults are the ones the contract documents", () => {
  // src/types.ts: backdropOpacity "0–1, defaults to 0.25", backdrop modelled
  // on Vimeo's controls with "background" as the ordinary case.
  assert.equal(DEFAULT_SETTINGS.subtitle.backdrop, "background");
  assert.equal(DEFAULT_SETTINGS.subtitle.backdropOpacity, 0.25);
  assert.equal(DEFAULT_SETTINGS.guideHours, 3);
  assert.ok(DEFAULT_SETTINGS.rewindMb >= REWIND_MIN);
  assert.ok(DEFAULT_SETTINGS.rewindMb <= REWIND_MAX);
});

test("the rewind buffer is clamped to the slider's range, whole megabytes", () => {
  assert.equal(clampSettings(settings({ rewindMb: 0 })).rewindMb, REWIND_MIN);
  assert.equal(clampSettings(settings({ rewindMb: 99_999 })).rewindMb, REWIND_MAX);
  // A value migrated from the old minutes setting is kept exactly, not
  // snapped to the slider's step.
  assert.equal(clampSettings(settings({ rewindMb: 546 })).rewindMb, 546);
  assert.equal(clampSettings(settings({ rewindMb: 700.4 })).rewindMb, 700);
  assert.equal(clampSettings(settings({ rewindMb: NaN })).rewindMb, REWIND_MIN);
});

test("what a buffer buys depends on the bitrate, and the arithmetic is bytes", () => {
  // 546 MB at 6.8 Mbps is the 640 s that "4 minutes" turned out to hold.
  assert.equal(Math.round(rewindSeconds(546, 6.8)), 642);
  assert.equal(rewindSeconds(512, 8), 512);
  // The same buffer lasts three times as long at a third of the rate.
  assert.equal(rewindSeconds(300, 3), 3 * rewindSeconds(300, 9));
  assert.equal(rewindSeconds(0, 8), 0);
  assert.equal(rewindSeconds(512, 0), 0);
  // Every listed bitrate is a positive, distinct, ascending figure.
  const rates = REWIND_BITRATES.map((b) => b.mbps);
  assert.deepEqual(rates, [...rates].sort((a, b) => a - b));
  assert.ok(rates.every((r) => r > 0));
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
  assert.deepEqual(planSave(before, settings({ rewindMb: 900 })), {
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

test("a setting at its default offers no reset; one moved does, for itself alone", () => {
  const s = settings({});
  for (const key of ["rewindMb", "hardwareDecode", "guideHours", "timeFormat", "subtitle.size", "subtitle.color"]) {
    assert.equal(isDefault(s, key), true, key);
  }
  const moved = settings({ rewindMb: 1024, guideHours: 5, subtitle: subtitle({ size: 50, color: "#f2e600" }) });
  assert.equal(isDefault(moved, "rewindMb"), false);
  assert.equal(isDefault(moved, "subtitle.size"), false);
  assert.equal(isDefault(moved, "hardwareDecode"), true);

  const back = { ...moved, ...resetPatch(moved, "subtitle.size") };
  assert.equal(back.subtitle.size, DEFAULT_SUBTITLE.size);
  // The rest of the style, and the other settings, stay where they were.
  assert.equal(back.subtitle.color, "#f2e600");
  assert.equal(back.rewindMb, 1024);
  assert.deepEqual(resetPatch(moved, "guideHours"), { guideHours: DEFAULT_SETTINGS.guideHours });
});

test("the backend's name for the default font counts as the default", () => {
  assert.equal(isDefault(settings({ subtitle: subtitle({ font: "sans-serif" }) }), "subtitle.font"), true);
  assert.equal(isDefault(settings({ subtitle: subtitle({ font: "Serif" }) }), "subtitle.font"), false);
});
