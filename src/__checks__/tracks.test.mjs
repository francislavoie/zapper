import test from "node:test";
import assert from "node:assert/strict";
import {
  ASPECT_CHOICES,
  DEINTERLACE_CHOICES,
  channelLayout,
  choiceLabel,
  codecName,
  hasAudioChoice,
  isPictureSubtitle,
  languageName,
  selectedLabel,
  trackLabel,
} from "../logic/tracks.ts";
import { subMenuPosition } from "../logic/favourites.ts";

const track = (t) => ({
  id: 1,
  title: null,
  lang: null,
  codec: null,
  channels: null,
  default: false,
  forced: false,
  hearingImpaired: false,
  visualImpaired: false,
  selected: false,
  ...t,
});

test("a language code becomes the language's name, whichever ISO code it is", () => {
  assert.equal(languageName("eng"), "English");
  assert.equal(languageName("en"), "English");
  assert.equal(languageName("fre"), "French");
  assert.equal(languageName("fra"), "French");
  assert.equal(languageName("ger"), "German");
  assert.equal(languageName("DEU"), "German");
  assert.equal(languageName(null), null);
  assert.equal(languageName("  "), null);
  // Something nobody knows is still something to choose by.
  assert.equal(languageName("qaa"), "QAA");
});

test("a track is named by its language, then its title, then its place", () => {
  assert.equal(trackLabel(track({ lang: "fre" }), 0).label, "French");
  assert.equal(trackLabel(track({ title: "Commentary" }), 0).label, "Commentary");
  assert.equal(trackLabel(track({}), 2).label, "Track 3");
});

test("the detail tells apart two tracks in one language", () => {
  const plain = trackLabel(track({ lang: "deu", codec: "aac", channels: 2 }), 0);
  const ad = trackLabel(track({ lang: "deu", codec: "aac", channels: 2, visualImpaired: true }), 1);
  assert.equal(plain.label, ad.label);
  assert.notEqual(plain.detail, ad.detail);
  assert.equal(plain.detail, "AAC · stereo");
  assert.equal(ad.detail, "Audio description · AAC · stereo");
  const titled = trackLabel(track({ lang: "eng", title: "Commentary", codec: "ac3", channels: 6 }), 0);
  assert.equal(titled.detail, "Commentary · Dolby Digital · 5.1");
  const sdh = trackLabel(track({ lang: "eng", codec: "dvb_teletext", hearingImpaired: true }), 0);
  assert.equal(sdh.detail, "SDH · Teletext");
});

test("codecs and channel counts are said in words", () => {
  assert.equal(codecName("eac3"), "Dolby Digital Plus");
  assert.equal(codecName("dvb_subtitle"), "DVB");
  assert.equal(codecName("vorbis"), "VORBIS");
  assert.equal(codecName(null), null);
  assert.equal(channelLayout(1), "mono");
  assert.equal(channelLayout(6), "5.1");
  assert.equal(channelLayout(4), "4 ch");
  assert.equal(channelLayout(null), null);
});

test("one audio track is no choice", () => {
  assert.ok(!hasAudioChoice({ audio: [] }));
  assert.ok(!hasAudioChoice({ audio: [track({})] }));
  assert.ok(hasAudioChoice({ audio: [track({ id: 1 }), track({ id: 2 })] }));
});

test("the row says which track is selected, or nothing", () => {
  const list = [track({ id: 1, lang: "eng" }), track({ id: 2, lang: "fre", selected: true })];
  assert.equal(selectedLabel(list), "French");
  assert.equal(selectedLabel([track({ lang: "eng" })]), null);
});

test("the fixed choices match the contract's values", () => {
  assert.deepEqual(
    ASPECT_CHOICES.map((c) => c.value),
    ["auto", "16:9", "4:3", "2.35:1", "fill", "stretch"],
  );
  assert.deepEqual(DEINTERLACE_CHOICES.map((c) => c.value), ["auto", "on", "off"]);
  assert.equal(choiceLabel(ASPECT_CHOICES, "fill"), "Fill (crop)");
});

test("a submenu opens to the right of its row, else to the left, and stays on screen", () => {
  const row = { left: 100, right: 300, top: 200 };
  // Room on the right: beside the row, first item level with it.
  assert.deepEqual(subMenuPosition(row, 180, 120, 1280, 800), { left: 302, top: 196 });
  // No room on the right: to the left of the parent menu.
  const nearEdge = { left: 1000, right: 1200, top: 200 };
  assert.deepEqual(subMenuPosition(nearEdge, 180, 120, 1280, 800), { left: 818, top: 196 });
  // No room on either side: pushed back inside the window.
  assert.equal(subMenuPosition({ left: 50, right: 250, top: 200 }, 180, 120, 300, 800).left, 114);
  // Near the bottom: slides up to fit.
  assert.equal(subMenuPosition({ left: 100, right: 300, top: 760 }, 180, 120, 1280, 800).top, 674);
});

test("subtitles drawn as pictures are told from text", () => {
  for (const codec of ["dvb_subtitle", "hdmv_pgs_subtitle", "dvd_subtitle", "picture"]) {
    assert.equal(isPictureSubtitle({ codec }), true, codec);
  }
  for (const codec of ["subrip", "ass", "webvtt", "mov_text", null]) {
    assert.equal(isPictureSubtitle({ codec }), false, String(codec));
  }
  assert.equal(codecName("picture"), "Pictures");
});
