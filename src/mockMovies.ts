// The mock's movie catalogue: 159,000 made-up titles over 40 categories, so
// the Movies tab is exercised at the scale a provider's catalogue really is.
//
// Nothing is built up front. A title is a pure function of its id (a hash,
// not an RNG stream, so any id can be made on its own), and a list is a scan
// over small typed arrays that are made the first time they are needed. A
// page of 60 rows costs 60 objects, however long the list is.
//
// The ids run newest first: id 1 is the latest addition, so "recently added"
// is id order and needs no sort. The titles are made of invented words; none
// is, or is meant to resemble, a real film.
//
// Flags in the URL: `?movies=none` is an account with no movies (a tuner),
// `?movies=empty` an Xtream server before its first movies phase, and
// `?movie=playing` boots the player already playing movie 1, for looking at
// the player before a band can start one.

import type {
  MarksDto,
  MovieDetailsDto,
  MovieDto,
  MoviePageDto,
  MovieQueryDto,
  MovieSort,
  MoviesSummaryDto,
  PlayerStateDto,
  VodCategoryDto,
} from "./types";

export const MOVIE_TOTAL = 159_000;
const CATEGORY_TOTAL = 40;
/** Titles added in the last thirty days. */
const RECENT_TOTAL = 240;
const DAY = 86_400;
/** What every mock film runs for, in seconds. */
const MOCK_DURATION = 6540;

const flag = (re: RegExp) => typeof location !== "undefined" && re.test(location.search);
const NONE = flag(/(^|[?&])movies=none(&|$)/);
const EMPTY = flag(/(^|[?&])movies=empty(&|$)/);
const HIDDEN = NONE || EMPTY;
const BOOT_PLAYING = flag(/(^|[?&])movie=playing(&|$)/);

const NOW = Math.floor(Date.now() / 1000);

// ------------------------------------------------------------------ made up

const ADJECTIVES = [
  "Silent", "Amber", "Hollow", "Gilded", "Restless", "Crooked", "Distant", "Velvet",
  "Paper", "Burning", "Quiet", "Wandering", "Frozen", "Hidden", "Scarlet", "Brittle",
  "Last", "Second", "Northern", "Salted", "Hungry", "Painted", "Narrow", "Wild",
  "Borrowed", "Copper", "Ashen", "Lonely", "Midnight", "Tangled", "Pale", "Glass",
];
const NOUNS = [
  "Lantern", "Harbour", "Orchard", "Compass", "Kestrel", "Meridian", "Cartographer", "Tide",
  "Furnace", "Almanac", "Signal", "Lighthouse", "Threshold", "Ferryman", "Quarry", "Mirage",
  "Chorus", "Embers", "Bridge", "Carnival", "Sparrow", "Anchor", "Ledger", "Winter",
  "Echo", "Garden", "Voyage", "Stranger", "Archive", "Postmark", "Harvest", "Tower",
];
const PLACES = [
  "Marrow Bay", "Fennick", "the Low Country", "Ostrava Pass", "Vellmar", "the Salt Road",
  "Halden", "Brindle Cross", "Corrin", "the Far Shore", "Tessaly", "Wick Harbour",
  "Ombrel", "the Iron Coast", "Lanthorn", "Kestrel Fell", "Dunmarrow", "the Glass Plains",
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
  "Drama", "Comedy", "Thriller", "Adventure", "Mystery", "Romance", "Fantasy",
  "Science fiction", "Crime", "Family", "War", "History",
];
const CATEGORY_NAMES = [
  "Action", "Adventure", "Animation", "Biography", "Comedy", "Crime", "Documentary",
  "Drama", "Family", "Fantasy", "History", "Horror", "Music", "Mystery", "Romance",
  "Science fiction", "Thriller", "War", "Western", "Classics",
  "4K Collection", "Recently Restored", "Festival Favourites", "Staff Picks",
  "Weekend Marathon", "Sleepers and Hidden Gems",
  "Award Winners, Nominees and Festival Favourites From Around the World",
  "Short Films", "Holiday", "Cult Cinema", "Foreign Language", "Late Night",
  "Coming of Age", "Heist", "Road Movies", "Courtroom", "Sports", "Musicals",
  "Silent Era", "Double Features",
];

/** A 32-bit hash of an id and a salt: independent draws per property, with no
 *  state, so any title can be made without its neighbours. */
function mix(i: number, salt: number): number {
  let h = Math.imul(i ^ Math.imul(salt + 1, 0x9e3779b1), 0x85ebca6b);
  h ^= h >>> 13;
  h = Math.imul(h, 0xc2b2ae35);
  h ^= h >>> 16;
  return h >>> 0;
}
const unit = (i: number, salt: number) => mix(i, salt) / 4294967296;
const choose = <T,>(xs: readonly T[], i: number, salt: number): T => xs[mix(i, salt) % xs.length];

function cleanNameOf(i: number): string {
  const adj = choose(ADJECTIVES, i, 10);
  const noun = choose(NOUNS, i, 11);
  const place = choose(PLACES, i, 12);
  switch (mix(i, 13) % 6) {
    case 0:
      return `${adj} ${noun}`;
    case 1:
      return `The ${noun} of ${place}`;
    case 2:
      return `${noun} ${place}`;
    case 3:
      return `${adj} ${noun} ${["II", "III", "Returns"][mix(i, 14) % 3]}`;
    case 4:
      return `${place.replace(/^the /, "")} ${noun}`;
    default:
      return `A ${adj} ${noun}`;
  }
}
const has4k = (i: number) => mix(i, 15) % 25 === 0;
const nameOf = (i: number) => (has4k(i) ? `${cleanNameOf(i)} 4K` : cleanNameOf(i));

const yearOf = (i: number) => 1960 + Math.floor(Math.pow(unit(i, 16), 0.55) * 67);
const ratingOf = (i: number): number | null =>
  unit(i, 17) < 0.08 ? null : Math.round((3 + unit(i, 18) * 6.2) * 10) / 10;
const durationOf = (i: number): number | null =>
  unit(i, 19) < 0.05 ? null : (70 + (mix(i, 20) % 100)) * 60;
const categoryNo = (i: number) => 1 + (mix(i, 21) % CATEGORY_TOTAL);

/** Newest first: the latest `RECENT_TOTAL` within the month, then the rest
 *  spread evenly back over three years. */
function addedOf(i: number): number {
  if (i <= RECENT_TOTAL) return NOW - 3600 - Math.floor(((i - 1) * 29 * DAY) / RECENT_TOTAL);
  const span = 3 * 365 * DAY - 30 * DAY;
  return NOW - 30 * DAY - Math.floor(((i - RECENT_TOTAL) * span) / (MOVIE_TOTAL - RECENT_TOTAL));
}

const svgUri = (svg: string) => `data:image/svg+xml,${encodeURIComponent(svg)}`;

function art(i: number, w: number, h: number, label: string): string {
  const hue = mix(i, 22) % 360;
  const initials = label
    .split(" ")
    .filter((w) => /^[A-Z]/.test(w))
    .slice(0, 2)
    .map((w) => w[0])
    .join("");
  return svgUri(
    `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">` +
      `<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">` +
      `<stop offset="0" stop-color="hsl(${hue},45%,34%)"/>` +
      `<stop offset="1" stop-color="hsl(${(hue + 50) % 360},55%,14%)"/></linearGradient></defs>` +
      `<rect width="${w}" height="${h}" fill="url(#g)"/>` +
      `<circle cx="${w * 0.7}" cy="${h * 0.3}" r="${h * 0.18}" fill="hsl(${hue},60%,60%)" opacity="0.25"/>` +
      `<text x="${w / 2}" y="${h * 0.58}" font-family="sans-serif" font-size="${h * 0.18}" ` +
      `fill="white" fill-opacity="0.8" text-anchor="middle">${initials}</text></svg>`,
  );
}

// -------------------------------------------------------------------- state

const favourites = new Set<number>();
const watched = new Set<number>();
const positions = new Map<number, number>();
/** Bumped by every mark, so a remembered list is not served stale. */
let marksVersion = 0;

for (let k = 1; k <= 12; k++) favourites.add(1 + ((k * 12_007) % MOVIE_TOTAL));
for (let k = 1; k <= 30; k++) watched.add(1 + ((k * 5_003) % MOVIE_TOTAL));

function row(i: number): MovieDto {
  const clean = cleanNameOf(i);
  return {
    movieId: i,
    name: nameOf(i),
    cleanName: clean,
    year: yearOf(i),
    rating: ratingOf(i),
    posterUrl: unit(i, 23) < 0.92 ? art(i, 200, 300, clean) : null,
    added: addedOf(i),
    durationSecs: durationOf(i),
    categoryId: `m${categoryNo(i)}`,
    favourite: favourites.has(i),
    watched: watched.has(i),
  };
}

// --------------------------------------------------------------- the indexes

let years: Uint16Array | null = null;
let ratings: Int16Array | null = null;
let names: string[] | null = null;
const orders = new Map<MovieSort, Uint32Array>();

function tables() {
  if (!years || !ratings) {
    years = new Uint16Array(MOVIE_TOTAL + 1);
    ratings = new Int16Array(MOVIE_TOTAL + 1);
    for (let i = 1; i <= MOVIE_TOTAL; i++) {
      years[i] = yearOf(i);
      const r = ratingOf(i);
      ratings[i] = r === null ? -1 : Math.round(r * 10);
    }
  }
  return { years, ratings };
}

function nameTable(): string[] {
  if (!names) {
    names = new Array<string>(MOVIE_TOTAL + 1);
    names[0] = "";
    for (let i = 1; i <= MOVIE_TOTAL; i++) names[i] = cleanNameOf(i).toLowerCase();
  }
  return names;
}

/** Every id in the order a sort wants, made once per sort. */
function orderFor(sort: MovieSort): Uint32Array {
  let o = orders.get(sort);
  if (o) return o;
  o = new Uint32Array(MOVIE_TOTAL);
  for (let k = 0; k < MOVIE_TOTAL; k++) o[k] = k + 1;
  if (sort === "az") {
    const n = nameTable();
    o.sort((a, b) => (n[a] < n[b] ? -1 : n[a] > n[b] ? 1 : a - b));
  } else if (sort === "year") {
    const { years: y } = tables();
    o.sort((a, b) => y[b] - y[a] || a - b);
  } else if (sort === "rating") {
    const { ratings: r } = tables();
    o.sort((a, b) => r[b] - r[a] || a - b);
  }
  orders.set(sort, o);
  return o;
}

let lastQuery: { key: string; ids: number[] } | null = null;

/** The ids a query lists, in order. The last answer is kept, since a list is
 *  read a page at a time. */
function listed(q: MovieQueryDto): number[] {
  const key = `${marksVersion}|${JSON.stringify(q)}`;
  if (lastQuery?.key === key) return lastQuery.ids;
  const { years: y } = tables();
  const from = q.yearFrom ?? -Infinity;
  const to = q.yearTo ?? Infinity;
  const ids: number[] = [];
  for (const id of orderFor(q.sort)) {
    if (y[id] < from || y[id] > to) continue;
    if (q.list === "recent" && id > RECENT_TOTAL) continue;
    if (q.list === "favourites" && !favourites.has(id)) continue;
    if (q.list === "watched" && !watched.has(id)) continue;
    if (q.list === "category" && `m${categoryNo(id)}` !== q.categoryId) continue;
    ids.push(id);
  }
  lastQuery = { key, ids };
  return ids;
}

let categoryList: VodCategoryDto[] | null = null;
function categories(): VodCategoryDto[] {
  if (!categoryList) {
    const counts = new Array<number>(CATEGORY_TOTAL + 1).fill(0);
    for (let i = 1; i <= MOVIE_TOTAL; i++) counts[categoryNo(i)]++;
    categoryList = CATEGORY_NAMES.map((name, k) => ({
      id: `m${k + 1}`,
      name,
      movieCount: counts[k + 1],
    }));
  }
  return categoryList;
}

const delay = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const inRange = (id: number) => Number.isInteger(id) && id >= 1 && id <= MOVIE_TOTAL;

function details(i: number): MovieDetailsDto {
  const movie = row(i);
  const person = (salt: number) =>
    `${choose(FIRST_NAMES, i, salt)} ${choose(LAST_NAMES, i, salt + 50)}`;
  const place = choose(PLACES, i, 30);
  const noun = choose(NOUNS, i, 31).toLowerCase();
  const adj = choose(ADJECTIVES, i, 32).toLowerCase();
  const date = `${movie.year}-${String(1 + (mix(i, 33) % 12)).padStart(2, "0")}-${String(1 + (mix(i, 34) % 28)).padStart(2, "0")}`;
  const langs = [["en"], ["en", "fr"], ["fr"], ["en", "de", "es"]][mix(i, 35) % 4];
  return {
    movie,
    plot:
      `When a ${adj} ${noun} turns up in ${place}, nobody there can say where it came from. ` +
      `${choose(FIRST_NAMES, i, 36)} follows it further than anyone expected, and finds that the town's quiet was never what it seemed.`,
    genres: [choose(GENRES, i, 37), choose(GENRES, i, 38)].filter((g, k, a) => a.indexOf(g) === k),
    director: person(40),
    cast: [41, 42, 43, 44, 45, 46].map(person),
    releaseDate: date,
    backdropUrls: [art(i, 640, 360, movie.cleanName)],
    tmdbId: 100_000 + i,
    audioLanguages: langs,
    subtitleLanguages: ["en", "fr", "de"].slice(0, 1 + (mix(i, 39) % 3)),
    fetchedAt: NOW,
  };
}

// ------------------------------------------------------------------- player

/** What the mock needs of its player, which lives in mock.ts. */
export interface MoviePlayerHost {
  state(): PlayerStateDto;
  patch(p: Partial<PlayerStateDto>): void;
  /** Starts a new open, so older timers and opens give way; returns its number. */
  open(): number;
  current(): number;
}

export function createMockMovies(host: MoviePlayerHost) {
  let clock: number | undefined;

  const onDemand = (id: number, position: number, buffered: boolean) => ({
    kind: "movie" as const,
    id,
    position,
    duration: MOCK_DURATION,
    bufferedTo: buffered ? Math.min(MOCK_DURATION, position + 120) : null,
  });

  /** The position follows the clock while the film plays. */
  function startClock() {
    if (clock !== undefined) return;
    clock = window.setInterval(() => {
      const p = host.state();
      const m = p.onDemand;
      if (p.state !== "playing" || m?.kind !== "movie") return;
      const position = Math.min(MOCK_DURATION, m.position + 1);
      positions.set(m.id, position);
      host.patch({ onDemand: onDemand(m.id, position, true) });
    }, 1000);
  }

  function seek(to: number) {
    const m = host.state().onDemand;
    if (m?.kind !== "movie") return;
    const position = Math.max(0, Math.min(MOCK_DURATION, to));
    positions.set(m.id, position);
    host.patch({ onDemand: onDemand(m.id, position, true) });
  }

  const playing = (id: number, position: number) =>
    host.patch({
      state: "playing",
      streamId: null,
      width: 1920,
      height: 1080,
      fps: 24,
      codec: "h264",
      error: null,
      ending: null,
      bufferedBack: 0,
      behindLive: 0,
      atLiveEdge: true,
      reconnect: null,
      onDemand: onDemand(id, position, true),
    });

  if (BOOT_PLAYING) {
    playing(1, 0);
    startClock();
  }

  return {
    moviesSummary: async (): Promise<MoviesSummaryDto> => {
      await delay(80);
      if (HIDDEN) {
        return {
          available: false,
          note: NONE
            ? "A tuner carries live channels only: it has no movies."
            : "No movies yet. They arrive with the catalogue's next refresh.",
          total: 0,
          favourites: 0,
          recent: 0,
          watched: 0,
          origin: "",
        };
      }
      return {
        available: true,
        note: null,
        total: MOVIE_TOTAL,
        favourites: favourites.size,
        recent: RECENT_TOTAL,
        watched: watched.size,
        origin: "mock",
      };
    },

    movieCategories: async (): Promise<VodCategoryDto[]> => {
      await delay(80);
      return HIDDEN ? [] : categories();
    },

    movies: async (query: MovieQueryDto, offset: number, limit: number): Promise<MoviePageDto> => {
      await delay(60);
      if (HIDDEN) return { total: 0, items: [] };
      const ids = listed(query);
      return { total: ids.length, items: ids.slice(offset, offset + limit).map(row) };
    },

    searchMovies: async (query: string, limit: number): Promise<MovieDto[]> => {
      await delay(60);
      const needle = query.trim().toLowerCase();
      if (HIDDEN || needle === "") return [];
      const n = nameTable();
      const out: MovieDto[] = [];
      for (let i = 1; i <= MOVIE_TOTAL && out.length < limit; i++) {
        if (n[i].includes(needle)) out.push(row(i));
      }
      return out;
    },

    movieDetails: async (movieId: number): Promise<MovieDetailsDto> => {
      await delay(250);
      if (HIDDEN || !inRange(movieId)) throw new Error("no such movie");
      return details(movieId);
    },

    similarMovies: async (movieId: number, limit: number): Promise<MovieDto[]> => {
      await delay(60);
      if (HIDDEN || !inRange(movieId)) return [];
      const category = categoryNo(movieId);
      const out: MovieDto[] = [];
      for (let i = movieId + 1; out.length < limit && i <= MOVIE_TOTAL; i++) {
        if (categoryNo(i) === category) out.push(row(i));
      }
      return out;
    },

    setMovieMarks: async (
      movieId: number,
      marks: { favourite?: boolean; watched?: boolean },
    ): Promise<MarksDto> => {
      await delay(40);
      if (!inRange(movieId)) throw new Error("no such movie");
      if (marks.favourite !== undefined) {
        if (marks.favourite) favourites.add(movieId);
        else favourites.delete(movieId);
      }
      if (marks.watched !== undefined) {
        if (marks.watched) watched.add(movieId);
        else watched.delete(movieId);
      }
      marksVersion++;
      return {
        id: movieId,
        favourite: favourites.has(movieId),
        watched: watched.has(movieId),
        position: positions.get(movieId) ?? null,
        duration: positions.has(movieId) ? MOCK_DURATION : null,
      };
    },

    /** Opens the film from `start`, or from the start, as the app does. */
    playMovie: async (movieId: number, start: number | null): Promise<void> => {
      if (!inRange(movieId)) throw new Error("no such movie");
      const mine = host.open();
      const from = Math.max(0, Math.min(MOCK_DURATION, start ?? 0));
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
        onDemand: onDemand(movieId, from, false),
      });
      await delay(700);
      if (host.current() !== mine) return;
      playing(movieId, from);
      startClock();
    },

    seekTo: async (seconds: number): Promise<void> => seek(seconds),
    seekRelative: (seconds: number): void => {
      const m = host.state().onDemand;
      if (m) seek(m.position + seconds);
    },
  };
}
