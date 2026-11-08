// Which languages to look for subtitles in, and how what OpenSubtitles
// answers reads.
//
// OpenSubtitles names languages in its own codes (`pt-br`, `zh-cn`), while
// the streams zapper plays, the settings' standing subtitle language and the
// interface's locales use ISO 639-2 (`fre`, `fra`) or BCP 47 (`en-CA`). The
// table here is the one place that maps between them. The standing choice
// (`subtitleLanguage`, see subtitles.ts) is kept as the viewer's track
// declared it; a downloaded track is tagged back in the same spelling when
// it is the same language, so that choosing it keeps the standing choice
// unchanged and an open finds it by `preferredIndex`.
//
// Pure. Checked in src/__checks__/onlineSubtitles.test.mjs.

import type {
  KeptSubtitleDto,
  SubtitleDownloadDto,
  SubtitleResultDto,
  SubtitleSearchDto,
  TrackDto,
} from "../types";
import { dateTime, hhmm } from "./format.ts";
import { languageName } from "./tracks.ts";

/** OpenSubtitles' language codes, each with its ISO 639-2 B and T codes
 *  (the first is the one a downloaded track is tagged with). */
export const OPENSUBTITLES_LANGUAGES: readonly { code: string; iso3: readonly string[] }[] = [
  { code: "en", iso3: ["eng"] },
  { code: "fr", iso3: ["fre", "fra"] },
  { code: "de", iso3: ["ger", "deu"] },
  { code: "es", iso3: ["spa"] },
  { code: "it", iso3: ["ita"] },
  { code: "pt-pt", iso3: ["por"] },
  { code: "pt-br", iso3: ["por"] },
  { code: "nl", iso3: ["dut", "nld"] },
  { code: "sv", iso3: ["swe"] },
  { code: "da", iso3: ["dan"] },
  { code: "no", iso3: ["nor", "nob", "nno"] },
  { code: "fi", iso3: ["fin"] },
  { code: "pl", iso3: ["pol"] },
  { code: "cs", iso3: ["cze", "ces"] },
  { code: "sk", iso3: ["slo", "slk"] },
  { code: "hu", iso3: ["hun"] },
  { code: "ro", iso3: ["rum", "ron"] },
  { code: "el", iso3: ["gre", "ell"] },
  { code: "tr", iso3: ["tur"] },
  { code: "ru", iso3: ["rus"] },
  { code: "uk", iso3: ["ukr"] },
  { code: "bg", iso3: ["bul"] },
  { code: "hr", iso3: ["hrv"] },
  { code: "sr", iso3: ["srp"] },
  { code: "sl", iso3: ["slv"] },
  { code: "et", iso3: ["est"] },
  { code: "lv", iso3: ["lav"] },
  { code: "lt", iso3: ["lit"] },
  { code: "he", iso3: ["heb"] },
  { code: "ar", iso3: ["ara"] },
  { code: "fa", iso3: ["per", "fas"] },
  { code: "hi", iso3: ["hin"] },
  { code: "zh-cn", iso3: ["chi", "zho"] },
  { code: "zh-tw", iso3: ["chi", "zho"] },
  { code: "ja", iso3: ["jpn"] },
  { code: "ko", iso3: ["kor"] },
  { code: "th", iso3: ["tha"] },
  { code: "vi", iso3: ["vie"] },
  { code: "id", iso3: ["ind"] },
  { code: "ms", iso3: ["may", "msa"] },
  { code: "ca", iso3: ["cat"] },
  { code: "eu", iso3: ["baq", "eus"] },
  { code: "gl", iso3: ["glg"] },
  { code: "is", iso3: ["ice", "isl"] },
];

/** Two-letter forms OpenSubtitles splits in two: the first listed is used. */
const BASE_ALIAS: Record<string, string> = { pt: "pt-pt", zh: "zh-cn" };

const known = (code: string) => OPENSUBTITLES_LANGUAGES.find((l) => l.code === code);

/** OpenSubtitles' code for a language as a stream, a setting or a locale
 *  spells it (`fre`, `fra`, `fr`, `FR`, `en-CA`); `null` when it has none. */
export function toSearchLanguage(code: string | null): string | null {
  const c = code?.trim().toLowerCase().replace(/_/g, "-");
  if (!c) return null;
  if (known(c)) return c;
  const byIso3 = OPENSUBTITLES_LANGUAGES.find((l) => l.iso3.includes(c));
  if (byIso3) return byIso3.code;
  const base = c.split("-")[0];
  if (base !== c) {
    if (known(base)) return base;
    if (BASE_ALIAS[base]) return BASE_ALIAS[base];
  }
  return BASE_ALIAS[c] ?? null;
}

/** The languages to search in, at most two, never none: the ones set in
 *  Settings when there are any; else the standing subtitle language, then
 *  the interface's first language that OpenSubtitles has; else English. */
export function searchLanguages(
  configured: readonly string[],
  standing: string | null,
  locales: readonly string[],
): string[] {
  const out: string[] = [];
  const add = (code: string | null) => {
    const l = toSearchLanguage(code);
    if (l && !out.includes(l)) out.push(l);
  };
  if (configured.some((c) => toSearchLanguage(c))) {
    configured.forEach(add);
  } else {
    add(standing);
    const locale = locales.find((l) => toSearchLanguage(l));
    if (locale) add(locale);
  }
  return out.length ? out.slice(0, 2) : ["en"];
}

/** The language a downloaded track is tagged with: the standing choice's own
 *  spelling when it is this language, else the ISO 639-2 code. */
export function trackLanguage(download: string, standing: string | null): string {
  if (standing && toSearchLanguage(standing) === toSearchLanguage(download)) return standing;
  return known(download.toLowerCase())?.iso3[0] ?? download;
}

/** A downloaded track's title in the picture menu. */
export function downloadTrackTitle(release: string): string {
  return "Downloaded · " + release.trim().slice(0, 120);
}

/** The kept subtitles not already loaded as a track of the stream. */
export function offeredDownloads(
  kept: readonly KeptSubtitleDto[],
  tracks: readonly TrackDto[],
): KeptSubtitleDto[] {
  const titles = new Set(tracks.map((t) => t.title));
  return kept.filter((k) => !titles.has(downloadTrackTitle(k.release)));
}

function downloadCount(n: number): string {
  if (n < 1000) return `${n} download${n === 1 ? "" : "s"}`;
  const k = n / 1000;
  return `${k >= 10 ? Math.round(k) : Math.round(k * 10) / 10}k downloads`;
}

/** A result as a list row: its language, and what tells it from the others. */
export function resultLine(r: SubtitleResultDto): { label: string; detail: string } {
  const parts = [
    r.release.trim(),
    r.hearingImpaired ? "SDH" : "",
    r.forced ? "Forced" : "",
    r.trusted ? "Trusted" : "",
    r.downloads > 0 ? downloadCount(r.downloads) : "",
    r.kept ? "On this computer" : "",
  ].filter(Boolean);
  return { label: languageName(r.language) ?? r.language, detail: parts.join(" · ") };
}

const OFF = "OpenSubtitles is off in Settings › Online services.";

const tryAt = (retryAt: number) => `OpenSubtitles asked zapper to wait. Try again at ${hhmm(retryAt)}.`;

function list(names: string[]): string {
  if (names.length < 2) return names.join("");
  return names.slice(0, -1).join(", ") + " and " + names[names.length - 1];
}

/** What a search says besides its results; `null` when it needs no words. */
export function searchNotice(s: SubtitleSearchDto, _now: number): string | null {
  switch (s.kind) {
    case "off":
      return OFF;
    case "unknownTitle":
      return "This title isn't in the catalogue, so there is nothing to search by.";
    case "busy":
      return tryAt(s.retryAt);
    case "results":
      if (s.results.length === 0) {
        return `Nothing found in ${list(s.languages.map((l) => languageName(l) ?? l))}.`;
      }
      return s.matchedBy === "name" ? "Matched by name: check the release fits this video." : null;
  }
}

const sameDay = (a: number, b: number) =>
  new Date(a * 1000).toDateString() === new Date(b * 1000).toDateString();

/** What a download says when it didn't give a subtitle. */
export function downloadNotice(d: SubtitleDownloadDto, now: number): string | null {
  switch (d.kind) {
    case "off":
      return OFF;
    case "busy":
      return tryAt(d.retryAt);
    case "quota": {
      const base = "Your OpenSubtitles downloads for today are used up.";
      if (d.resetAt === null) return base;
      return `${base} More from ${sameDay(d.resetAt, now) ? hhmm(d.resetAt) : dateTime(d.resetAt)}.`;
    }
    default:
      return null;
  }
}

/** The heading of the subtitle finder: the film, or the show and episode. */
export function findHeading(
  movie: { cleanName: string } | null,
  episode: {
    series: { cleanName: string };
    episode: { season: number; episode: number };
  } | null,
): string {
  if (movie) return movie.cleanName;
  if (episode) {
    return `${episode.series.cleanName} · S${episode.episode.season} E${episode.episode.episode}`;
  }
  return "";
}
