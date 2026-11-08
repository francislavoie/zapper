// The mock's show catalogue: 12,000 made-up shows over 30 categories, with
// their seasons and episodes made the first time a show is asked about, so
// the Shows tab is exercised at a provider's scale and with the awkward
// shapes a provider sends.
//
// As in mockMovies.ts, nothing is built up front: a show is a pure function
// of its id (a seeded hash, so any id can be made on its own), and its
// episodes are a pure function of the show's id. Only the marks (favourite,
// watched, where an episode was left) are state, kept in memory. Ids run
// newest first: id 1 is the latest addition. Every name is made of invented
// words; none is, or is meant to resemble, a real show. One in four starts
// with a language, as some providers' do ("EN - "), for the search.
//
// A few shows are fixed, for looking at particular cases:
//   1  three seasons (3, 8 and 6 episodes); season 1 watched, S2 E1–E3
//      watched and S2 E4 left at 70 %, so S2 E4 is next up;
//   2  32 seasons, the first with 400 episodes (a long list, windowed);
//   3  two specials, and a two-part entry listed once as S1 E1–E2;
//   4  no episodes at all.
// An episode's id is (show id + 1) × 100,000 + season × 1,000 + number:
// 201,001 and up, clear of every mock movie id.
//
// Flags in the URL: `?shows=none` is an account with no shows (a tuner),
// `?show=playing` boots the player already playing show 1's S2 E4, and
// `?episode=ending` boots it playing S2 E4 eighteen seconds from its end,
// where Up next counts down; `&drop=1` makes the stream fail twelve seconds
// from an episode's end, as a connection broken off would (Retry). An
// episode that plays to its end stops there, finished.

import type { MoviePlayerHost } from "./mockMovies";
import type {
  EpisodeDto,
  MarksDto,
  MovieSort,
  OnDemandDto,
  PlayingEpisodeDto,
  SeasonDto,
  SeriesCategoryDto,
  SeriesDetailsDto,
  SeriesDto,
  SeriesPageDto,
  SeriesQueryDto,
  ShowsSummaryDto,
} from "./types";
import { titleMatches, titleNeedles } from "./logic/titleMatch";

export const SHOW_TOTAL = 12_000;
const CATEGORY_TOTAL = 30;
/** Shows added in the last thirty days. */
const RECENT_TOTAL = 120;
const DAY = 86_400;
/** What an episode with no length of its own runs for, in the player. */
const MOCK_EPISODE_SECS = 2700;
/** The in-progress and watched fractions (catalog/vod_progress.rs). */
const STARTED_FRACTION = 0.01;
const WATCHED_FRACTION = 0.92;

const flag = (re: RegExp) => typeof location !== "undefined" && re.test(location.search);
const NONE = flag(/(^|[?&])shows=none(&|$)/);
const BOOT_PLAYING = flag(/(^|[?&])show=playing(&|$)/);
const BOOT_ENDING = flag(/(^|[?&])episode=ending(&|$)/);
const DROP = flag(/(^|[?&])drop=1(&|$)/);
/** How far from its end a `drop=1` episode fails. */
const DROP_BEFORE_END_SECS = 12;

const NOW = Math.floor(Date.now() / 1000);
const NOW_YEAR = new Date(NOW * 1000).getFullYear();

// ------------------------------------------------------------------ made up

const OPENERS = [
  "Harbour", "Lantern", "Kestrel", "Copper", "Meridian", "Saltmarsh", "Ferrow", "Hollin",
  "Bramble", "Tidewater", "Quarry", "Thistle", "Ashgrove", "Wexley", "Marrow", "Corvel",
  "Larkspur", "Gannet", "Ivory", "Sable", "Tallow", "Juniper", "Cinder", "Oriel",
];
const CLOSERS = [
  "Street", "Lights", "House", "Station", "Parish", "Watch", "Crossing", "Yard",
  "Ward", "Point", "Hollow", "Row", "Landing", "Circle", "Heights", "Reach",
];
const SINGLES = [
  "Undertow", "Outliers", "The Wardens", "Small Hours", "Second Shift", "The Tallies",
  "Driftwood", "The Lockwardens", "Low Tide", "Foxglove", "The Lamplighters", "Wayfinders",
  "Paper Kingdoms", "Lodestar", "The Understudies", "Quiet Waters",
];
const EPISODE_WORDS_A = [
  "Night", "First", "Hollow", "Open", "Last", "Cold", "Broken", "Second", "Long",
  "Silver", "Narrow", "Distant", "Burning", "Quiet", "Borrowed", "Missing",
];
const EPISODE_WORDS_B = [
  "Shift", "Light", "Water", "Ground", "Hours", "Signal", "Harbour", "Letters",
  "Bridge", "Ledger", "Debts", "Doors", "Tides", "Season", "Promises", "Orbit",
];
const FIRST_NAMES = [
  "Alex", "Morgan", "Sasha", "Jordan", "Robin", "Casey", "Noa", "Idris", "Mira", "Tobias",
  "Lena", "Rafael", "Ines", "Callum", "Yara", "Emil", "Petra", "Soren", "Amara", "Dario",
];
const LAST_NAMES = [
  "Rowan", "Alder", "Voss", "Marlow", "Quill", "Ashby", "Brandt", "Okafor", "Lindqvist",
  "Varga", "Hartley", "Moreau", "Castellan", "Dunne", "Ferro", "Halloran", "Imrie", "Kovac",
];
const GENRES = [
  "Drama", "Comedy", "Crime", "Mystery", "Thriller", "Fantasy", "Science fiction",
  "Family", "History", "Documentary", "Romance", "Adventure",
];
const CATEGORY_NAMES = [
  "Drama", "Comedy", "Crime", "Mystery", "Thriller", "Science fiction", "Fantasy",
  "Documentary", "Nature", "History", "Kids", "Animation", "Reality", "Cooking", "Travel",
  "Talk", "Sport", "Medical", "Courtroom", "Period drama", "Mini-series", "Sitcoms",
  "Anthology", "Teen", "Westerns", "Workplace comedies", "Detective stories",
  "Limited Series, Long-Running Dramas and Award Winners From Around the World",
  "Classics", "Late night",
];

/** A 32-bit hash of an id and a salt: a seeded draw per property, with no
 *  state, so any show or episode can be made without its neighbours. */
function mix(i: number, salt: number): number {
  let h = Math.imul(i ^ Math.imul(salt + 1, 0x9e3779b1), 0x85ebca6b);
  h ^= h >>> 13;
  h = Math.imul(h, 0xc2b2ae35);
  h ^= h >>> 16;
  return h >>> 0;
}
const unit = (i: number, salt: number) => mix(i, salt) / 4294967296;
const choose = <T,>(xs: readonly T[], i: number, salt: number): T => xs[mix(i, salt) % xs.length];

/** The language some providers put before a title: one show in four has
 *  one, the fixed shows (1 to 4) aside. */
const LANGUAGES = ["EN - ", "EN - ", "FR - ", "FR/EN - "];
function languageOf(i: number): string {
  const k = mix(i, 140) % 16;
  return i > 4 && k < LANGUAGES.length ? LANGUAGES[k] : "";
}

function cleanNameOf(i: number): string {
  return languageOf(i) + titleOf(i);
}

function titleOf(i: number): string {
  switch (mix(i, 110) % 4) {
    case 0:
      return `${choose(OPENERS, i, 111)} ${choose(CLOSERS, i, 112)}`;
    case 1:
      return `The ${choose(OPENERS, i, 113)} ${choose(CLOSERS, i, 114)}`;
    case 2:
      return choose(SINGLES, i, 115) + (mix(i, 116) % 3 === 0 ? ` of ${choose(OPENERS, i, 117)}` : "");
    default:
      return `${choose(OPENERS, i, 118)} & ${choose(OPENERS, i, 119)}`;
  }
}
const has4k = (i: number) => mix(i, 120) % 30 === 0;
const nameOf = (i: number) => (has4k(i) ? `${cleanNameOf(i)} 4K` : cleanNameOf(i));
const yearOf = (i: number) => 1990 + Math.floor(Math.pow(unit(i, 121), 0.6) * 37);
const ratingOf = (i: number): number | null =>
  unit(i, 122) < 0.1 ? null : Math.round((4 + unit(i, 123) * 5.4) * 10) / 10;
const categoryNo = (i: number) => 1 + (mix(i, 124) % CATEGORY_TOTAL);

/** Newest first: the latest `RECENT_TOTAL` within the month, then the rest
 *  spread evenly back over three years. */
function addedOf(i: number): number {
  if (i <= RECENT_TOTAL) return NOW - 5400 - Math.floor(((i - 1) * 29 * DAY) / RECENT_TOTAL);
  const span = 3 * 365 * DAY - 30 * DAY;
  return NOW - 30 * DAY - Math.floor(((i - RECENT_TOTAL) * span) / (SHOW_TOTAL - RECENT_TOTAL));
}

const svgUri = (svg: string) => `data:image/svg+xml,${encodeURIComponent(svg)}`;

function art(i: number, w: number, h: number, label: string): string {
  const hue = mix(i, 125) % 360;
  const initials = label
    .split(" ")
    .filter((x) => /^[A-Z0-9]/.test(x))
    .slice(0, 2)
    .map((x) => x[0])
    .join("");
  return svgUri(
    `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">` +
      `<defs><linearGradient id="g" x1="0" y1="1" x2="1" y2="0">` +
      `<stop offset="0" stop-color="hsl(${hue},40%,16%)"/>` +
      `<stop offset="1" stop-color="hsl(${(hue + 70) % 360},50%,36%)"/></linearGradient></defs>` +
      `<rect width="${w}" height="${h}" fill="url(#g)"/>` +
      `<rect x="${w * 0.12}" y="${h * 0.62}" width="${w * 0.76}" height="${h * 0.04}" fill="hsl(${hue},60%,70%)" opacity="0.3"/>` +
      `<text x="${w / 2}" y="${h * 0.52}" font-family="sans-serif" font-size="${Math.min(w, h) * 0.24}" ` +
      `fill="white" fill-opacity="0.8" text-anchor="middle">${initials}</text></svg>`,
  );
}

// ----------------------------------------------------------------- episodes

/** An episode's place in its show, before any marks. */
interface Slot {
  episodeId: number;
  season: number;
  episode: number;
  episodeEnd: number | null;
}

const episodeIdOf = (seriesId: number, season: number, n: number) =>
  (seriesId + 1) * 100_000 + season * 1000 + n;

/** The show and place an episode id stands for, or null for one no show has. */
function slotOf(episodeId: number): { seriesId: number; slot: Slot } | null {
  if (!Number.isInteger(episodeId)) return null;
  const seriesId = Math.floor(episodeId / 100_000) - 1;
  if (!inRange(seriesId)) return null;
  const slot = slots(seriesId).find((s) => s.episodeId === episodeId);
  return slot ? { seriesId, slot } : null;
}

/** How many episodes each season has, from season 1; the specials apart. */
function shape(id: number): { seasons: number[]; specials: number } {
  switch (id) {
    case 1:
      return { seasons: [3, 8, 6], specials: 0 };
    case 2:
      return {
        seasons: Array.from({ length: 32 }, (_, k) => (k === 0 ? 400 : 6 + (mix(id * 64 + k, 130) % 8))),
        specials: 0,
      };
    case 3:
      return { seasons: [7, 9, 8, 10], specials: 2 };
    case 4:
      return { seasons: [], specials: 0 };
    default:
      return {
        seasons: Array.from({ length: 1 + (id % 6) }, (_, k) => 6 + (mix(id * 64 + k, 130) % 8)),
        specials: id % 5 === 0 ? 1 + (mix(id, 131) % 3) : 0,
      };
  }
}

const slotCache = new Map<number, Slot[]>();

/** A show's episodes in the order the details list them: by season, the
 *  specials last, then by number. */
function slots(id: number): Slot[] {
  const kept = slotCache.get(id);
  if (kept) return kept;
  const { seasons, specials } = shape(id);
  const out: Slot[] = [];
  seasons.forEach((count, k) => {
    const season = k + 1;
    for (let n = 1; n <= count; n++) {
      // Show 3's opening two-parter is one entry for both.
      if (id === 3 && season === 1 && n === 2) continue;
      const end = id === 3 && season === 1 && n === 1 ? 2 : null;
      out.push({ episodeId: episodeIdOf(id, season, n), season, episode: n, episodeEnd: end });
    }
  });
  for (let n = 1; n <= specials; n++) {
    out.push({ episodeId: episodeIdOf(id, 0, n), season: 0, episode: n, episodeEnd: null });
  }
  slotCache.set(id, out);
  return out;
}

const durationOf = (seriesId: number, e: number): number | null =>
  seriesId !== 1 && unit(e, 132) < 0.05 ? null : (22 + (mix(e, 133) % 39)) * 60;
const lengthOf = (seriesId: number, e: number) => durationOf(seriesId, e) ?? MOCK_EPISODE_SECS;

// -------------------------------------------------------------------- state

const favourites = new Set<number>();
/** Episode id → when it became watched (an order, not a time). */
const watchedAt = new Map<number, number>();
/** Episode id → where it was left, seconds from the start. */
const positions = new Map<number, number>();
/** Episode id → when it last played (an order, not a time). */
const playedAt = new Map<number, number>();
/** Shows with an episode marked: the only ones that can be watched. */
const touched = new Set<number>();
/** Shows whose episodes have been seen, which is when their counts are known. */
const seen = new Set<number>();
let tick = 0;
/** Bumped by every mark, so a remembered list is not served stale. */
let marksVersion = 0;

for (let k = 1; k <= 9; k++) favourites.add(1 + ((k * 1_307) % SHOW_TOTAL));
favourites.add(1);
for (let i = 1; i <= SHOW_TOTAL; i++) if (i % 4 !== 0 || i <= 4) seen.add(i);

function watch(episodeId: number, seriesId: number) {
  if (!watchedAt.has(episodeId)) watchedAt.set(episodeId, ++tick);
  touched.add(seriesId);
}

// Show 1: season 1 and S2 E1–E3 watched, S2 E4 left at 70 %.
for (const s of slots(1)) {
  if (s.season === 1 || (s.season === 2 && s.episode <= 3)) watch(s.episodeId, 1);
}
{
  const e = episodeIdOf(1, 2, 4);
  positions.set(e, Math.round(lengthOf(1, e) * 0.7));
  playedAt.set(e, ++tick);
}
// A few shows seen to the end.
for (const id of [9, 23, 57, 88, 140, 301]) {
  for (const s of slots(id)) if (s.season !== 0) watch(s.episodeId, id);
}

const isWatched = (seriesId: number): boolean => {
  if (!touched.has(seriesId)) return false;
  const regular = slots(seriesId).filter((s) => s.season !== 0);
  return regular.length > 0 && regular.every((s) => watchedAt.has(s.episodeId));
};

function row(i: number): SeriesDto {
  const clean = cleanNameOf(i);
  const known = seen.has(i);
  const { seasons } = shape(i);
  return {
    seriesId: i,
    name: nameOf(i),
    cleanName: clean,
    year: yearOf(i),
    rating: ratingOf(i),
    posterUrl: unit(i, 126) < 0.92 ? art(i, 200, 300, titleOf(i)) : null,
    added: addedOf(i),
    categoryId: `s${categoryNo(i)}`,
    seasonCount: known ? seasons.length : null,
    episodeCount: known ? slots(i).length : null,
    favourite: favourites.has(i),
    watched: isWatched(i),
  };
}

function airYear(seriesId: number, season: number): number {
  return Math.min(NOW_YEAR, yearOf(seriesId) + Math.max(0, season - 1));
}

function episode(seriesId: number, s: Slot): EpisodeDto {
  const e = s.episodeId;
  const titled = seriesId === 1 || mix(e, 134) % 9 !== 0;
  const title = titled ? `${choose(EPISODE_WORDS_A, e, 135)} ${choose(EPISODE_WORDS_B, e, 136)}` : null;
  const month = 1 + ((s.episode - 1 + (mix(seriesId, 137) % 6)) % 12);
  return {
    episodeId: e,
    seriesId,
    season: s.season,
    episode: s.episode,
    episodeEnd: s.episodeEnd,
    title,
    plot:
      unit(e, 138) < 0.1
        ? null
        : `${choose(FIRST_NAMES, e, 139)} finds a ${choose(EPISODE_WORDS_A, e, 140).toLowerCase()} ` +
          `${choose(EPISODE_WORDS_B, e, 141).toLowerCase()} where nobody expected one, and the rest ` +
          `of the week goes nothing like the plan.`,
    durationSecs: durationOf(seriesId, e),
    stillUrl: unit(e, 142) < 0.94 ? art(e, 320, 180, title ?? `E ${s.episode}`) : null,
    airDate: `${airYear(seriesId, s.season)}-${String(month).padStart(2, "0")}-${String(1 + (mix(e, 143) % 28)).padStart(2, "0")}`,
    watched: watchedAt.has(e),
    position: positions.get(e) ?? null,
    duration: playedAt.has(e) ? lengthOf(seriesId, e) : null,
  };
}

/** What is next for a show: the episode in progress that played last, else
 *  the next unwatched after the last one watched, else the first unwatched;
 *  never a special (series_store.rs `next_up`, in brief). */
function nextUp(eps: readonly EpisodeDto[]): number | null {
  const order = eps
    .filter((e) => e.season !== 0)
    .sort((a, b) => a.season - b.season || a.episode - b.episode || a.episodeId - b.episodeId);
  const started = (e: EpisodeDto) => {
    const len = e.duration ?? e.durationSecs;
    if (e.position === null || !len) return false;
    const f = e.position / len;
    return f >= STARTED_FRACTION && f < WATCHED_FRACTION;
  };
  let inProgress: EpisodeDto | null = null;
  for (const e of order) {
    if (e.watched || !started(e)) continue;
    if (!inProgress || (playedAt.get(e.episodeId) ?? -1) >= (playedAt.get(inProgress.episodeId) ?? -1)) {
      inProgress = e;
    }
  }
  if (inProgress) return inProgress.episodeId;
  let last: EpisodeDto | null = null;
  for (const e of order) {
    if (!e.watched) continue;
    if (!last || (watchedAt.get(e.episodeId) ?? -1) >= (watchedAt.get(last.episodeId) ?? -1)) last = e;
  }
  const w: EpisodeDto | null = last;
  const after = w
    ? order.find((e) => {
        const end = Math.max(w.episode, w.episodeEnd ?? w.episode);
        return !e.watched && (e.season > w.season || (e.season === w.season && e.episode > end));
      })
    : undefined;
  return (after ?? order.find((e) => !e.watched))?.episodeId ?? null;
}

function seasonsOf(eps: readonly EpisodeDto[]): SeasonDto[] {
  const by = new Map<number, SeasonDto>();
  for (const e of eps) {
    const s = by.get(e.season) ?? {
      number: e.season,
      name: e.season === 0 ? "Specials" : `Season ${e.season}`,
      episodeCount: 0,
      watchedCount: 0,
    };
    s.episodeCount++;
    if (e.watched) s.watchedCount++;
    by.set(e.season, s);
  }
  return [...by.values()].sort((a, b) => (a.number === 0 ? 1 : b.number === 0 ? -1 : a.number - b.number));
}

function details(i: number): SeriesDetailsDto {
  seen.add(i);
  const series = row(i);
  const eps = slots(i).map((s) => episode(i, s));
  const years = eps.map((e) => Number(e.airDate?.slice(0, 4))).filter((y) => y > 0);
  const person = (salt: number) =>
    `${choose(FIRST_NAMES, i, salt)} ${choose(LAST_NAMES, i, salt + 50)}`;
  return {
    series,
    plot:
      `In ${choose(OPENERS, i, 150)} ${choose(CLOSERS, i, 151).toLowerCase()}, ${choose(FIRST_NAMES, i, 152)} ` +
      `keeps the place running while everyone around them keeps secrets of their own.`,
    genres: [choose(GENRES, i, 153), choose(GENRES, i, 154)].filter((g, k, a) => a.indexOf(g) === k),
    director: mix(i, 155) % 3 === 0 ? null : person(156),
    cast: [157, 158, 159, 160, 161].map(person),
    releaseDate: `${series.year}-0${1 + (mix(i, 162) % 9)}-1${mix(i, 163) % 9}`,
    backdropUrls: [art(i, 640, 360, titleOf(i))],
    tmdbId: 300_000 + i,
    firstYear: years.length ? Math.min(...years) : series.year,
    lastYear: years.length ? Math.max(...years) : null,
    seasons: seasonsOf(eps),
    episodes: eps,
    nextUp: nextUp(eps),
    fetchedAt: NOW,
  };
}

// --------------------------------------------------------------- the lists

let names: string[] | null = null;
const orders = new Map<MovieSort, Uint32Array>();

function nameTable(): string[] {
  if (!names) {
    names = new Array<string>(SHOW_TOTAL + 1);
    names[0] = "";
    for (let i = 1; i <= SHOW_TOTAL; i++) names[i] = cleanNameOf(i).toLowerCase();
  }
  return names;
}

function orderFor(sort: MovieSort): Uint32Array {
  let o = orders.get(sort);
  if (o) return o;
  o = new Uint32Array(SHOW_TOTAL);
  for (let k = 0; k < SHOW_TOTAL; k++) o[k] = k + 1;
  if (sort === "az") {
    const n = nameTable();
    o.sort((a, b) => (n[a] < n[b] ? -1 : n[a] > n[b] ? 1 : a - b));
  } else if (sort === "year") {
    o.sort((a, b) => yearOf(b) - yearOf(a) || a - b);
  } else if (sort === "rating") {
    o.sort((a, b) => (ratingOf(b) ?? -1) - (ratingOf(a) ?? -1) || a - b);
  }
  orders.set(sort, o);
  return o;
}

let lastQuery: { key: string; ids: number[] } | null = null;

function listed(q: SeriesQueryDto): number[] {
  const key = `${marksVersion}|${JSON.stringify(q)}`;
  if (lastQuery?.key === key) return lastQuery.ids;
  const from = q.yearFrom ?? -Infinity;
  const to = q.yearTo ?? Infinity;
  const ids: number[] = [];
  const needles = titleNeedles(q.search ?? "");
  const n = needles.length > 0 ? nameTable() : null;
  for (const id of orderFor(q.sort)) {
    const y = yearOf(id);
    if (y < from || y > to) continue;
    if (n && !titleMatches(needles, n[id])) continue;
    if (q.list === "recent" && id > RECENT_TOTAL) continue;
    if (q.list === "favourites" && !favourites.has(id)) continue;
    if (q.list === "watched" && !isWatched(id)) continue;
    if (q.list === "category" && `s${categoryNo(id)}` !== q.categoryId) continue;
    ids.push(id);
  }
  lastQuery = { key, ids };
  return ids;
}

let categoryList: SeriesCategoryDto[] | null = null;
function categories(): SeriesCategoryDto[] {
  if (!categoryList) {
    const counts = new Array<number>(CATEGORY_TOTAL + 1).fill(0);
    for (let i = 1; i <= SHOW_TOTAL; i++) counts[categoryNo(i)]++;
    categoryList = CATEGORY_NAMES.map((name, k) => ({
      id: `s${k + 1}`,
      name,
      seriesCount: counts[k + 1],
    }));
  }
  return categoryList;
}

const delay = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const inRange = (id: number) => Number.isInteger(id) && id >= 1 && id <= SHOW_TOTAL;

function episodeMarks(seriesId: number, episodeId: number): MarksDto {
  return {
    id: episodeId,
    favourite: false,
    watched: watchedAt.has(episodeId),
    position: positions.get(episodeId) ?? null,
    duration: playedAt.has(episodeId) ? lengthOf(seriesId, episodeId) : null,
  };
}

// ------------------------------------------------------------------- player

export function createMockShows(host: MoviePlayerHost) {
  let clock: number | undefined;

  const onDemand = (id: number, position: number, buffered: boolean) => {
    const at = slotOf(id);
    const duration = at ? lengthOf(at.seriesId, id) : MOCK_EPISODE_SECS;
    return {
      kind: "episode" as const,
      id,
      position,
      duration,
      bufferedTo: buffered ? Math.min(duration, position + 120) : null,
    };
  };

  /** Where an episode is, as the player would save it: past the watched
   *  fraction it counts as watched. */
  function record(id: number, position: number) {
    positions.set(id, position);
    const at = slotOf(id);
    if (at && !watchedAt.has(id) && position >= lengthOf(at.seriesId, id) * WATCHED_FRACTION) {
      watch(id, at.seriesId);
      marksVersion++;
    }
  }

  /** The position follows the clock while an episode plays. At its end it
   *  stops, finished; with `drop=1` the stream fails short of that. */
  function startClock() {
    if (clock !== undefined) return;
    clock = window.setInterval(() => {
      const p = host.state();
      const m = p.onDemand;
      if (p.state !== "playing" || m?.kind !== "episode") return;
      const length = m.duration ?? MOCK_EPISODE_SECS;
      const position = Math.min(length, m.position + 1);
      if (DROP && position >= length - DROP_BEFORE_END_SECS) {
        host.open();
        host.patch({
          state: "failed",
          error: "the stream broke off",
          ending: { kind: "network", status: null, limit: false, slots: null, clipSecs: null },
          reconnect: null,
          bufferedBack: 0,
          behindLive: 0,
          onDemand: onDemand(m.id, position, false),
          finished: null,
        });
        return;
      }
      record(m.id, position);
      if (position >= length) {
        host.open();
        host.patch({
          state: "stopped",
          streamId: null,
          width: null,
          height: null,
          fps: null,
          codec: null,
          error: null,
          ending: null,
          reconnect: null,
          onDemand: null,
          finished: onDemand(m.id, position, false),
        });
        return;
      }
      host.patch({ onDemand: onDemand(m.id, position, true) });
    }, 1000);
  }

  function seek(to: number) {
    const m = host.state().onDemand;
    if (m?.kind !== "episode") return;
    const position = Math.max(0, Math.min(m.duration ?? MOCK_EPISODE_SECS, to));
    record(m.id, position);
    host.patch({ onDemand: onDemand(m.id, position, true) });
  }

  const playing = (id: number, position: number) =>
    host.patch({
      state: "playing",
      streamId: null,
      width: 1920,
      height: 1080,
      fps: 25,
      codec: "h264",
      error: null,
      ending: null,
      bufferedBack: 0,
      behindLive: 0,
      atLiveEdge: true,
      reconnect: null,
      onDemand: onDemand(id, position, true),
      finished: null,
    });

  if (BOOT_PLAYING || BOOT_ENDING) {
    const id = episodeIdOf(1, 2, 4);
    const at = BOOT_ENDING ? lengthOf(1, id) - 18 : (positions.get(id) ?? 0);
    playing(id, at);
    startClock();
  }

  /** Opens an episode from `from`, playing. */
  const open = async (episodeId: number, seriesId: number, from: number) => {
    const mine = host.open();
    playedAt.set(episodeId, ++tick);
    touched.add(seriesId);
    host.patch({
      state: "opening",
      streamId: null,
      error: null,
      ending: null,
      bufferedBack: 0,
      behindLive: 0,
      liveJump: null,
      joiningLive: false,
      reconnect: null,
      onDemand: onDemand(episodeId, from, false),
      finished: null,
    });
    await delay(700);
    if (host.current() !== mine) return;
    playing(episodeId, from);
    startClock();
  };

  /** Up next, as the app's player has it: only from the episode open or
   *  the one that just played to its end, which is marked watched; the
   *  next opens from its start. Each next episode asked for is noted in
   *  `__mockNextAsked`, for a driver to see none was after a drop. */
  const playNextEpisode = async (fromId: number, nextId: number): Promise<void> => {
    const g = globalThis as Record<string, unknown>;
    g.__mockNextAsked = [...((g.__mockNextAsked as number[] | undefined) ?? []), nextId];
    if (fromId === nextId) throw new Error("that is the episode playing");
    const p = host.state();
    const is = (o: OnDemandDto | null) => o?.kind === "episode" && o.id === fromId;
    const allowed =
      p.state === "playing" || p.state === "paused"
        ? is(p.onDemand)
        : p.state === "stopped" && is(p.finished);
    if (!allowed) throw new Error("that episode is not the one playing");
    const from = slotOf(fromId);
    const at = slotOf(nextId);
    if (!from || !at) throw new Error("no such episode");
    watch(fromId, from.seriesId);
    marksVersion++;
    await open(nextId, at.seriesId, 0);
  };

  const playEpisode = async (episodeId: number, start: number | null): Promise<void> => {
    const at = slotOf(episodeId);
    if (!at) throw new Error("no such episode");
    const from = Math.max(0, Math.min(lengthOf(at.seriesId, episodeId), start ?? 0));
    await open(episodeId, at.seriesId, from);
  };

  return {
    showsSummary: async (): Promise<ShowsSummaryDto> => {
      await delay(80);
      if (NONE) {
        return {
          available: false,
          note: "A tuner carries live channels only: it has no shows.",
          total: 0,
          favourites: 0,
          recent: 0,
          watched: 0,
          origin: "",
        };
      }
      let watched = 0;
      for (const id of touched) if (isWatched(id)) watched++;
      return {
        available: true,
        note: null,
        total: SHOW_TOTAL,
        favourites: favourites.size,
        recent: RECENT_TOTAL,
        watched,
        origin: "mock",
      };
    },

    seriesCategories: async (): Promise<SeriesCategoryDto[]> => {
      await delay(80);
      return NONE ? [] : categories();
    },

    series: async (query: SeriesQueryDto, offset: number, limit: number): Promise<SeriesPageDto> => {
      await delay(60);
      if (NONE) return { total: 0, items: [] };
      const ids = listed(query);
      return { total: ids.length, items: ids.slice(offset, offset + limit).map(row) };
    },

    seriesDetails: async (seriesId: number): Promise<SeriesDetailsDto> => {
      await delay(250);
      if (NONE || !inRange(seriesId)) throw new Error("no such show");
      return details(seriesId);
    },

    setSeriesMarks: async (
      seriesId: number,
      marks: { favourite?: boolean; watched?: boolean },
    ): Promise<MarksDto> => {
      await delay(40);
      if (!inRange(seriesId)) throw new Error("no such show");
      if (marks.favourite !== undefined) {
        if (marks.favourite) favourites.add(seriesId);
        else favourites.delete(seriesId);
      }
      if (marks.watched === true) {
        for (const s of slots(seriesId)) if (s.season !== 0) watch(s.episodeId, seriesId);
      } else if (marks.watched === false) {
        for (const s of slots(seriesId)) {
          watchedAt.delete(s.episodeId);
          positions.delete(s.episodeId);
        }
      }
      if (marks.watched !== undefined) seen.add(seriesId);
      marksVersion++;
      return {
        id: seriesId,
        favourite: favourites.has(seriesId),
        watched: isWatched(seriesId),
        position: null,
        duration: null,
      };
    },

    setEpisodeMarks: async (episodeId: number, watched: boolean): Promise<MarksDto> => {
      await delay(40);
      const at = slotOf(episodeId);
      if (!at) throw new Error("no such episode");
      if (watched) {
        watch(episodeId, at.seriesId);
      } else {
        watchedAt.delete(episodeId);
        positions.delete(episodeId);
      }
      marksVersion++;
      return episodeMarks(at.seriesId, episodeId);
    },

    playingEpisode: async (episodeId: number): Promise<PlayingEpisodeDto> => {
      await delay(60);
      const at = slotOf(episodeId);
      if (!at) throw new Error("no such episode");
      return { series: row(at.seriesId), episode: episode(at.seriesId, at.slot) };
    },

    /** Opens the episode from `start`, or from the start, as the app does. */
    playEpisode,
    playNextEpisode,

    seekTo: async (seconds: number): Promise<void> => seek(seconds),
    seekRelative: (seconds: number): void => {
      const m = host.state().onDemand;
      if (m) seek(m.position + seconds);
    },
  };
}
