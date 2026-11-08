// OpenSubtitles' languages, the search's choice of them, result lines and
// the sentences for refusals.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  OPENSUBTITLES_LANGUAGES,
  downloadNotice,
  downloadTrackTitle,
  findHeading,
  offeredDownloads,
  resultLine,
  searchLanguages,
  searchNotice,
  toSearchLanguage,
  trackLanguage,
} from "../logic/onlineSubtitles.ts";
import { dateTime, setTimeFormat } from "../logic/format.ts";

const at = (h, m, day = 4) => new Date(2027, 2, day, h, m).getTime() / 1000;
const NOW = at(12, 0);
const WAIT = "OpenSubtitles asked zapper to wait. Try again at 14:05.";
const SPENT = "Your OpenSubtitles downloads for today are used up.";

test("the language table", () => {
  assert.equal(OPENSUBTITLES_LANGUAGES.length, 44);
  assert.deepEqual(OPENSUBTITLES_LANGUAGES.find((l) => l.code === "fr").iso3, ["fre", "fra"]);
});

test("toSearchLanguage", () => {
  for (const c of ["fre", "fra", "fr", "FR", "fr-CA"]) assert.equal(toSearchLanguage(c), "fr");
  assert.equal(toSearchLanguage("por"), "pt-pt");
  assert.equal(toSearchLanguage("pt-BR"), "pt-br");
  assert.equal(toSearchLanguage("zho"), "zh-cn");
  for (const c of ["nb", "nb-NO", "nn", "no"]) assert.equal(toSearchLanguage(c), "no");
  for (const c of ["zh-Hant", "zh-TW", "zh-HK", "zh-MO", "zh-Hant-TW"]) assert.equal(toSearchLanguage(c), "zh-tw");
  for (const c of ["zh", "zh-CN", "zh-Hans"]) assert.equal(toSearchLanguage(c), "zh-cn");
  assert.equal(toSearchLanguage("pt"), "pt-pt");
  assert.equal(toSearchLanguage("pt-XX"), "pt-pt");
  assert.equal(toSearchLanguage("xx"), null);
  assert.equal(toSearchLanguage(null), null);
  assert.equal(toSearchLanguage(""), null);
});

test("searchLanguages", () => {
  assert.deepEqual(searchLanguages([], "fre", ["en-CA"]), ["fr", "en"]);
  assert.deepEqual(searchLanguages(["de"], "fre", []), ["de"]);
  assert.deepEqual(searchLanguages([], null, []), ["en"]);
  assert.deepEqual(searchLanguages([], "fre", ["fr-FR", "en"]), ["fr"]);
  assert.deepEqual(searchLanguages(["de", "es", "it"], null, []), ["de", "es"]);
  assert.deepEqual(searchLanguages(["xx"], null, ["xx", "nl-BE"]), ["nl"]);
  assert.deepEqual(searchLanguages([], "qaa", ["xx"]), ["en"]);
});

test("trackLanguage", () => {
  assert.equal(trackLanguage("fr", "fra"), "fra");
  assert.equal(trackLanguage("fr", "eng"), "fre");
  assert.equal(trackLanguage("pt-br", null), "por");
  assert.equal(trackLanguage("xx", null), "xx");
});

test("downloadTrackTitle", () => {
  assert.equal(downloadTrackTitle("Film.0001.2021.1080p.TEST"), "Downloaded · Film.0001.2021.1080p.TEST");
  assert.equal(downloadTrackTitle("x".repeat(200)).length, "Downloaded · ".length + 120);
});

test("offeredDownloads hides what is loaded", () => {
  const a = { fileId: 1, language: "en", release: "A", hearingImpaired: false, downloadedAt: 1 };
  const b = { ...a, fileId: 2, release: "B" };
  const tracks = [{ id: 1, title: downloadTrackTitle("A") }];
  assert.deepEqual(offeredDownloads([a, b], tracks), [b]);
  assert.deepEqual(offeredDownloads([a, b], []), [a, b]);
});

test("resultLine", () => {
  const r = {
    fileId: 1,
    language: "en",
    release: "Film.0001.2021.1080p.TEST",
    hearingImpaired: true,
    forced: false,
    trusted: true,
    downloads: 1200,
    kept: false,
  };
  assert.deepEqual(resultLine(r), {
    label: "English",
    detail: "Film.0001.2021.1080p.TEST · SDH · Trusted · 1.2k downloads",
  });
  assert.equal(
    resultLine({ ...r, release: "", hearingImpaired: false, trusted: false, forced: true, downloads: 0, kept: true })
      .detail,
    "Forced · On this computer",
  );
  assert.match(resultLine({ ...r, downloads: 15000 }).detail, /15k downloads/);
  assert.match(resultLine({ ...r, downloads: 950 }).detail, /950 downloads/);
});

test("searchNotice", () => {
  setTimeFormat("24h");
  try {
    searchNoticeChecks();
  } finally {
    setTimeFormat("auto");
  }
});

function searchNoticeChecks() {
  assert.equal(searchNotice({ kind: "off" }, NOW), "OpenSubtitles is off in Settings › Online services.");
  assert.equal(
    searchNotice({ kind: "unknownTitle" }, NOW),
    "This title isn't in the catalogue, so there is nothing to search by.",
  );
  assert.equal(searchNotice({ kind: "busy", retryAt: at(14, 5) }, NOW), WAIT);
  const res = { kind: "results", results: [], languages: ["en", "fr"], matchedBy: "id" };
  assert.equal(searchNotice(res, NOW), "Nothing found in English and French.");
  const one = { fileId: 1, language: "en", release: "r", hearingImpaired: false, forced: false, trusted: false, downloads: 0, kept: false };
  assert.equal(
    searchNotice({ ...res, results: [one], matchedBy: "name" }, NOW),
    "Matched by name: check the release fits this video.",
  );
  assert.equal(searchNotice({ ...res, results: [one] }, NOW), null);
  assert.equal(searchNotice({ ...res, languages: [] }, NOW), "Nothing found.");
  assert.equal(searchNotice({ ...res, matchedBy: "name" }, NOW), "Nothing found in English and French.");
}

test("downloadNotice", () => {
  setTimeFormat("24h");
  try {
    downloadNoticeChecks();
  } finally {
    setTimeFormat("auto");
  }
});

function downloadNoticeChecks() {
  assert.equal(downloadNotice({ kind: "off" }, NOW), "OpenSubtitles is off in Settings › Online services.");
  assert.equal(downloadNotice({ kind: "busy", retryAt: at(14, 5) }, NOW), WAIT);
  assert.equal(
    downloadNotice({ kind: "quota", resetAt: at(18, 30) }, NOW),
    "Your OpenSubtitles downloads for today are used up. More from 18:30.",
  );
  assert.equal(
    downloadNotice({ kind: "quota", resetAt: at(18, 30, 6) }, NOW),
    `${SPENT} More from ${dateTime(at(18, 30, 6))}.`,
  );
  assert.equal(downloadNotice({ kind: "quota", resetAt: null }, NOW), SPENT);
  assert.equal(downloadNotice({ kind: "signIn", refused: false, email: false }, NOW), null);
}

test("findHeading", () => {
  assert.equal(findHeading({ cleanName: "Film 0001" }, null), "Film 0001");
  assert.equal(
    findHeading(null, { series: { cleanName: "Show 0001" }, episode: { season: 1, episode: 2 } }),
    "Show 0001 · S1 E2",
  );
  assert.equal(findHeading(null, null), "");
});

// ---- the finder's state (src/onlineSubtitles.ts)
import {
  clearKept, closeFindSubtitles, currentFind, keptChanged, keptFor, loadKept, openFindSubtitles,
  setKeptSource,
} from "../onlineSubtitles.ts";

test("the finder opens on a title and closes", () => {
  assert.equal(currentFind(), null);
  openFindSubtitles({ kind: "movie", id: 7 });
  assert.deepEqual(currentFind(), { kind: "movie", id: 7 });
  closeFindSubtitles();
  assert.equal(currentFind(), null);
});
test("the kept cache is keyed by kind and id, and kept for 20 titles", async () => {
  clearKept();
  const calls = [];
  setKeptSource(async (kind, id) => {
    calls.push(`${kind}:${id}`);
    return [{ fileId: id, language: "en", release: kind, hearingImpaired: false, downloadedAt: 1 }];
  });
  await loadKept({ kind: "movie", id: 7 });
  await loadKept({ kind: "episode", id: 7 });
  await loadKept({ kind: "movie", id: 7 });
  assert.deepEqual(calls, ["movie:7", "episode:7"]);
  assert.equal(keptFor({ kind: "movie", id: 7 })[0].release, "movie");
  assert.equal(keptFor({ kind: "episode", id: 7 })[0].release, "episode");
  keptChanged({ kind: "movie", id: 7 });
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(calls.length, 3);
  for (let i = 100; i < 120; i++) await loadKept({ kind: "movie", id: i });
  assert.equal(keptFor({ kind: "movie", id: 7 }), undefined);
  assert.ok(keptFor({ kind: "movie", id: 119 }));
  setKeptSource();
  clearKept();
});
