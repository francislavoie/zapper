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
  isSignedIn,
  signInLine,
  keptTrack,
  offeredDownloads,
  resultLine,
  searchLanguages,
  searchNotice,
  toSearchLanguage,
  trackLanguage,
} from "../logic/onlineSubtitles.ts";
import { dateTime, hhmm, setTimeFormat } from "../logic/format.ts";

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
  const tracks = [{ id: 1, title: downloadTrackTitle("A"), lang: "eng" }];
  assert.deepEqual(offeredDownloads([a, b], tracks), [b]);
  assert.deepEqual(offeredDownloads([a, b], []), [a, b]);
});

// English and French files are often made for the same release.
const en = { fileId: 7000001, language: "en", release: "Film.0001.2021.1080p.TEST", hearingImpaired: false, downloadedAt: 1 };
const fr = { ...en, fileId: 7000004, language: "fr" };

test("a kept file is its loaded track by release and language", () => {
  const enTrack = { id: 3, title: downloadTrackTitle(en.release), lang: "eng" };
  assert.equal(keptTrack(en, [enTrack]), enTrack);
  assert.equal(keptTrack(fr, [enTrack]), undefined);
  // Tagged in the standing choice's own spelling, or in either ISO code.
  for (const lang of ["fre", "fra", "fr"]) {
    const frTrack = { id: 4, title: downloadTrackTitle(fr.release), lang };
    assert.equal(keptTrack(fr, [enTrack, frTrack]), frTrack);
  }
  // A stream's own track of that language isn't the file.
  assert.equal(keptTrack(fr, [{ id: 1, title: "French", lang: "fre" }]), undefined);
  // A language OpenSubtitles has no code for is compared as written.
  const xx = { ...en, language: "xx" };
  assert.equal(keptTrack(xx, [{ ...enTrack, lang: "xx" }])?.id, 3);
  assert.equal(keptTrack(xx, [enTrack]), undefined);
});

test("with English loaded, French of the same release is still offered", () => {
  const tracks = [{ id: 3, title: downloadTrackTitle(en.release), lang: "eng" }];
  assert.deepEqual(offeredDownloads([en, fr], tracks), [fr]);
  const both = [...tracks, { id: 4, title: downloadTrackTitle(fr.release), lang: "fre" }];
  assert.deepEqual(offeredDownloads([en, fr], both), []);
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
  currentServices, loadServices, setKeptSource, setServices, setServicesSource,
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

const deferred = () => {
  let resolve;
  const promise = new Promise((r) => (resolve = r));
  return { promise, resolve };
};
const kept1 = (release) => [{ fileId: 1, language: "en", release, hearingImpaired: false, downloadedAt: 1 }];

test("a slow earlier read landing after a newer one is dropped", async () => {
  clearKept();
  const slow = deferred();
  const fast = deferred();
  const queue = [slow, fast];
  setKeptSource(() => queue.shift().promise);
  const t = { kind: "movie", id: 5 };
  const first = loadKept(t);
  const second = loadKept(t, true);
  fast.resolve(kept1("new"));
  await second;
  slow.resolve(kept1("old"));
  await first;
  assert.equal(keptFor(t)[0].release, "new");
  setKeptSource();
  clearKept();
});
test("reads of one title at the same time share one request", async () => {
  clearKept();
  let calls = 0;
  const d = deferred();
  setKeptSource(() => (calls++, d.promise));
  const t = { kind: "episode", id: 9 };
  const a = loadKept(t);
  const b = loadKept(t);
  d.resolve(kept1("x"));
  await Promise.all([a, b]);
  assert.equal(calls, 1);
  setKeptSource();
  clearKept();
});

const services = (user) => ({
  openSubtitles: { enabled: true, languages: [], signedInAs: user, signInRefused: false, downloadsLeft: null, downloadsResetAt: null },
});
test("a settings read still in flight does not overwrite a newer answer", async () => {
  const d = deferred();
  setServicesSource(() => d.promise);
  const load = loadServices();
  setServices(services("tester"));
  d.resolve(services(null));
  await load;
  assert.equal(currentServices().data.openSubtitles.signedInAs, "tester");
  setServicesSource();
});

// ---- Settings › Online services: the sign-in line

const status = (over) => ({
  enabled: true,
  languages: [],
  signedInAs: "tester",
  signInRefused: false,
  downloadsLeft: null,
  downloadsResetAt: null,
  ...over,
});

test("the sign-in line counts what is left, in the singular for one", () => {
  const now = at(9, 0);
  assert.equal(signInLine(status({}), now), "Signed in as tester");
  assert.equal(
    signInLine(status({ downloadsLeft: 17 }), now),
    "Signed in as tester · 17 downloads left today",
  );
  assert.equal(
    signInLine(status({ downloadsLeft: 1 }), now),
    "Signed in as tester · 1 download left today",
  );
});

test("with none left, the line says when there will be more", () => {
  setTimeFormat("24h");
  const now = at(9, 0);
  assert.equal(
    signInLine(status({ downloadsLeft: 0, downloadsResetAt: at(18, 30) }), now),
    `Signed in as tester · no downloads left until ${hhmm(at(18, 30))}`,
  );
  assert.equal(
    signInLine(status({ downloadsLeft: 0, downloadsResetAt: at(18, 30, 5) }), now),
    `Signed in as tester · no downloads left until ${dateTime(at(18, 30, 5))}`,
  );
  assert.equal(
    signInLine(status({ downloadsLeft: 0 }), now),
    "Signed in as tester · no downloads left today",
  );
});

test("a sign-in OpenSubtitles refused is not a sign-in, whoever it names", () => {
  assert.equal(isSignedIn(status({})), true);
  assert.equal(isSignedIn(status({ signInRefused: true })), false);
  assert.equal(isSignedIn(status({ signedInAs: null })), false);
});
