import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  MEDIA_SERVER,
  NAMING_EXAMPLES,
  NAMING_PRESETS,
  TOKENS,
  TOKEN_HELP,
  clean,
  patternFor,
  place,
  placePath,
  previewPath,
  strmFile,
  validatePattern,
} from "../logic/naming.ts";

// The cases the recording service's own tests read too, so the preview and
// the names on disk cannot drift apart.
const cases = JSON.parse(
  readFileSync(new URL("../../src-tauri/src/recording/naming_cases.json", import.meta.url), "utf8"),
);

const civil = ([year, month, day, hour, minute]) => ({ year, month, day, hour, minute });

test("the shared cases render as the service renders them", () => {
  for (const c of cases.render) {
    assert.equal(placePath(place(c.pattern, c.programme, civil(c.start))), c.path, c.pattern);
  }
});

test("the shared cases are checked as the service checks them", () => {
  for (const c of cases.validate) {
    assert.equal(validatePattern(c.pattern), c.error, c.pattern);
  }
});

test("the shared .strm names are the service's", () => {
  assert.ok(cases.strm.length > 0);
  for (const c of cases.strm) {
    assert.equal(strmFile(c.base), c.file, c.base);
    assert.equal(clean(c.file, 255), c.file, "a name every file system accepts");
  }
});

const AT = civil([2026, 11, 3, 20, 0]);
const harbour = NAMING_EXAMPLES[0].programme;
const news = { title: "The Evening News", channel: "Meridian 24" };
const path = (pattern, p) => placePath(place(pattern, p, AT));

test("every preset, with and without an episode", () => {
  const byId = Object.fromEntries(NAMING_PRESETS.map((p) => [p.id, p.pattern]));
  assert.equal(byId.mediaServer, MEDIA_SERVER);
  assert.equal(path(byId.mediaServer, harbour), "Harbour Lights/Harbour Lights - S02E04 - 2026-11-03 20.00");
  assert.equal(path(byId.mediaServer, news), "The Evening News/The Evening News - 2026-11-03 20.00");
  assert.equal(path(byId.byChannel, harbour), "Northwind One HD/Harbour Lights - 2026-11-03 20.00");
  assert.equal(path(byId.byDate, news), "2026-11-03/20.00 Meridian 24 - The Evening News");
  assert.equal(path(byId.flat, news), "The Evening News - Meridian 24 - 2026-11-03 20.00");
  for (const p of NAMING_PRESETS) {
    if (p.pattern) assert.equal(validatePattern(p.pattern), null, p.id);
  }
});

test("every token has a value and a line of help", () => {
  for (const t of TOKENS) {
    assert.ok(TOKEN_HELP[t], t);
    assert.notEqual(path(`x {${t}}`, harbour), "x", t);
  }
  assert.equal(path("{se}", { ...news, season: 3 }), "The Evening News - 2026-11-03 20.00");
});

test("empty tokens take their separators along", () => {
  assert.equal(path("{show} - {se} - {date}", news), "The Evening News - 2026-11-03");
  assert.equal(path("{se} - {show}", news), "The Evening News");
  assert.equal(path("{show} ({title}) {date}", news), "The Evening News 2026-11-03");
  assert.equal(path("{show}/Season {season}/{show}", news), "The Evening News/Season/The Evening News");
  assert.equal(path("{show} ({year}) [{se}]", harbour), "Harbour Lights (2026) [S02E04]");
});

test("nothing leads out of the output folder", () => {
  for (const p of ["../{show}", "/etc/{show}", "C:\\{show}", "{show}/../x", "CON/{show}"]) {
    assert.ok(validatePattern(p), p);
    assert.equal(path(p, news), "The Evening News/The Evening News - 2026-11-03 20.00", p);
  }
  assert.equal(patternFor("custom", "../x"), MEDIA_SERVER);
  assert.equal(patternFor("custom", " {date}/{show} "), "{date}/{show}");
  assert.equal(patternFor("flat", "{date}/{show}"), NAMING_PRESETS[3].pattern);
  assert.equal(clean("nul.mkv", 120), "nul_.mkv");
  assert.equal(clean(".hidden", 120), "hidden");
});

test("long names are cut in bytes", () => {
  const long = "Q".repeat(500);
  const pl = place("{show}/{show} - {title} - {date} {time}", { title: long, episodeTitle: long }, AT);
  assert.equal(pl.folders[0].length, 100);
  assert.ok(new TextEncoder().encode(pl.base).length <= 180);
  assert.equal(new TextEncoder().encode(clean("é".repeat(300), 101)).length, 100);
});

test("the preview is the whole path, in the folder's own style", () => {
  const { programme, start } = NAMING_EXAMPLES[0];
  assert.equal(
    previewPath("/home/me/Videos/zapper/", MEDIA_SERVER, programme, start),
    "/home/me/Videos/zapper/Harbour Lights/Harbour Lights - S02E04 - 2026-11-03 20.00.mkv",
  );
  assert.equal(
    previewPath("D:\\TV", "{show} {date}", NAMING_EXAMPLES[1].programme, NAMING_EXAMPLES[1].start),
    "D:\\TV\\The Evening News 2026-11-04.mkv",
  );
});
