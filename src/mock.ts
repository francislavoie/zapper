// A fake backend, used only when Tauri is absent (i.e. `vite dev` in a plain
// browser). It exists so the UI can be driven and screenshotted without mpv or
// the Rust side, and so layout is exercised at a realistic scale — tens of
// thousands of channels across hundreds of categories is the whole reason the
// list is virtualized, and a 20-row fixture would hide every bug that matters.
//
// Shapes here mirror src/types.ts exactly. Nothing in this file is imported by
// the Tauri path at runtime beyond the module load itself.

import { auditLightSurfaces, auditOverVideo } from "./audit";
import { alternatives } from "./logic/searchQuery";
import { rankAlternatives } from "./logic/alternatives";
import { patternFor, place, validatePattern } from "./logic/naming";
import { parsePort, passwordProblem } from "./logic/network";
import { afterChoosing, openChoice, type SubtitlePreference } from "./logic/subtitles";
import { createMockContinue } from "./mockContinue";
import { createMockMovies } from "./mockMovies";
import { createMockShows } from "./mockShows";
import type {
  AccountCandidateDto,
  AccountDto,
  ActivityRowDto,
  BackupFileDto,
  CategoryDto,
  ChannelDto,
  ChannelQualityDto,
  FavouriteDto,
  GuideRowDto,
  MediaKey,
  MediaSessionDto,
  PlayerStateDto,
  PlayerOptionsDto,
  ProgrammeDto,
  TrackDto,
  AspectMode,
  DeinterlaceMode,
  ProgrammeHitDto,
  QualityDto,
  NetworkDto,
  NetworkSettingsDto,
  BackupDto,
  BackupsDto,
  RecordingDto,
  ConflictDto,
  RecordingSettingsDto,
  RecordingStatusDto,
  ScheduleRequest,
  ServiceStatusDto,
  SettingsDto,
  SubtitleStyleDto,
  SyncStatusDto,
  InstallKind,
  InstallOutcomeDto,
  InstallWhen,
  UpdateReleaseDto,
  UpdateStatusDto,
  VideoRect,
} from "./types";

const CHANNEL_TOTAL = 60000;
const CATEGORY_TOTAL = 800;
/** Roughly one row in sixty is a provider-injected divider. */
const SEPARATOR_EVERY = 60;

// ---------------------------------------------------------------- randomness

/** mulberry32: deterministic, so two runs screenshot the same thing. */
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const rand = rng(0x7a99e2);
const pick = <T,>(xs: readonly T[]): T => xs[Math.floor(rand() * xs.length)];

// ------------------------------------------------------------------ vocabulary

const COUNTRIES = [
  "CA",
  "US",
  "UK",
  "FR",
  "DE",
  "ES",
  "IT",
  "NL",
  "PT",
  "BR",
  "MX",
  "AU",
  "IN",
  "PL",
  "SE",
] as const;

const TOPICS = [
  "ENTERTAINMENT",
  "SPORTS",
  "NEWS",
  "MOVIES",
  "KIDS",
  "DOCUMENTARY",
  "MUSIC",
  "LIFESTYLE",
  "RELIGIOUS",
  "LOCAL",
  "TRAVEL",
  "SERIES",
  "RADIO",
  "WEATHER",
  "SHOPPING",
  "CLASSIC TV",
] as const;

const NETWORKS = [
  "NORTHWIND NEWS",
  "HARBOUR SPORTS 1",
  "HARBOUR SPORTS 2",
  "LUMEN KIDS",
  "COBALT MOVIES",
  "COBALT MOVIES 2",
  "MERIDIAN ONE",
  "MERIDIAN TWO",
  "SILVERLINE",
  "CHANNEL 9 NORTH",
  "PEAK SPORTS MAIN EVENT",
  "ORBIT SPORTS",
  "FIELDHOUSE",
  "NORTHWIND BUSINESS",
  "LEDGER TV",
  "DEEP FIELD",
  "ANNALS",
  "WILD ATLAS",
  "AMBER CLASSICS",
  "TIDE",
  "VELVET",
  "SATIN CINEMA",
  "STARLING",
  "EMBER MOVIES",
  "PEBBLE",
  "DOODLE",
  "SPROUT TOWN",
  "PULSE",
  "ECHO HITS",
  "CHUCKLE",
  "SIMMER",
  "HEARTH & HOME",
  "LIFELINE",
  "NEBULA",
  "CROSSROADS",
  "SUMMIT",
  "AURORA",
  "CANAL VERT",
  "RHEIN EINS",
  "WELLE 7",
  "LA ONDA",
  "TERRA UNO",
  "MAREA",
  "KANAAL ZUID",
  "BRUG 6",
  "SOLARIS",
  "CORAZÓN",
  "SITARA",
  "ALPINE SPORT",
  "ARENA PLUS",
] as const;

const SUFFIXES = ["", "", "", "", " HD", " FHD", " 4K", " RAW", " EAST", " WEST"];

const PROGRAMMES = [
  "Evening News",
  "Harbour League: Live",
  "Tonight with Sam Avery",
  "Documentary: Deep Ocean",
  "Movie: The Long Road",
  "Local Weather",
  "Morning Show",
  "Classic Match Replay",
  "Wildlife Rescue",
  "Kitchen Rescue",
  "Top 40 Countdown",
  "Grand Prix: Practice 2",
  "Hockey: Third Period",
  "Breaking News Special",
  // Accented, so a search for "cafe" has something to fold onto.
  "Le Café des Étoiles",
] as const;

// -------------------------------------------------------------- data building

export interface MockCatalog {
  categories: CategoryDto[];
  /** Every row, separators included, grouped by category in rail order. */
  rows: ChannelDto[];
  /** categoryId -> [start, length) into `rows`. */
  span: Map<string, [number, number]>;
}

/** A second, independent roll per channel, from its id. What the first
 *  catalogue had is drawn from `rand()`, and drawing more from it would move
 *  every channel after the first that asked; this leaves them where they are. */
function idRoll(streamId: number, salt: number): number {
  const x = Math.imul(streamId ^ salt, 2654435761) >>> 0;
  return (x % 10007) / 10007;
}

const FAIL_REASONS = [
  "refused by the provider (HTTP 403)",
  "not on the server (HTTP 404)",
  "the server answered with something that is not video",
  "no picture arrived",
];

function makeQuality(name: string, streamId: number, nowSec: number): QualityDto | null {
  // A "4K" in the name means nothing, so the mock carries one channel that
  // claims it and delivers 720p, for the badge to be checked against.
  if (name === "CA: HARBOUR SPORTS 1 4K") {
    return {
      codec: "h264",
      width: 1280,
      height: 720,
      fps: 59.94,
      interlaced: false,
      isHdr: false,
      audioCodec: "aac",
      audioChannels: 2,
      measuredAt: nowSec - 3 * 3600,
      okCount: 12,
      failCount: 1,
      failedAt: null,
      failReason: null,
    };
  }
  // Some channels did not load last time they were tried: dimmed in the
  // list, and in the mock they fail once more when played, then work.
  const failed =
    idRoll(streamId, 0x5eed) < 0.03
      ? {
          failedAt: nowSec - Math.floor(idRoll(streamId, 7) * 9 * 86400),
          failReason: FAIL_REASONS[Math.floor(idRoll(streamId, 11) * FAIL_REASONS.length)],
        }
      : null;
  // Most channels have never been opened. `null` is unknown, not bad.
  if (rand() > 0.09) {
    if (!failed) return null;
    return {
      codec: null,
      width: null,
      height: null,
      fps: null,
      interlaced: null,
      isHdr: false,
      audioCodec: null,
      audioChannels: null,
      measuredAt: null,
      okCount: 0,
      failCount: 1,
      ...failed,
    };
  }
  const r = rand();
  const [w, h] =
    r < 0.5 ? [1920, 1080] : r < 0.85 ? [1280, 720] : r < 0.95 ? [3840, 2160] : [720, 576];
  const surround = idRoll(streamId, 3) < 0.3;
  return {
    codec: rand() < 0.75 ? "h264" : "hevc",
    width: w,
    height: h,
    fps: rand() < 0.6 ? 50 : 59.94,
    // Standard definition is broadcast interlaced, and plenty of 1080 too.
    interlaced: h === 576 || (h === 1080 && idRoll(streamId, 5) < 0.3),
    isHdr: h >= 2160 && rand() < 0.4,
    audioCodec: surround ? "eac3" : "aac",
    audioChannels: surround ? 6 : 2,
    measuredAt: nowSec - Math.floor(idRoll(streamId, 13) * 40 * 86400),
    okCount: Math.floor(rand() * 40),
    failCount: rand() < 0.15 ? Math.floor(rand() * 20) : 0,
    failedAt: null,
    failReason: null,
    ...failed,
  };
}

function makeProgramme(nowSec: number): ProgrammeDto {
  const slot = 1800 + Math.floor(rand() * 4) * 900;
  const start = nowSec - Math.floor(rand() * slot);
  return {
    start,
    stop: start + slot,
    title: pick(PROGRAMMES),
    description:
      rand() < 0.6
        ? "A description the provider supplied, of roughly the length these actually run to in the wild."
        : null,
  };
}

/** A whole schedule for one channel, tiled from its now-playing slot.
 *
 *  A channel with no `nowPlaying` gets nothing at all: most of a large
 *  catalogue typically has no EPG, and the grid has to show that honestly rather than inventing rows
 *  it would never have in the wild. */
function schedule(row: ChannelDto, from: number, to: number): ProgrammeDto[] {
  const seed = row.nowPlaying;
  if (!seed) return [];
  const dur = Math.max(900, seed.stop - seed.start);
  // Align the tiling to the seed so the grid and the channel row agree about
  // what is on now.
  const first = seed.start - Math.ceil(Math.max(0, seed.start - from) / dur) * dur;
  const out: ProgrammeDto[] = [];
  for (let start = first; start < to; start += dur) {
    if (start + dur <= from) continue;
    const slot = Math.round((start - seed.start) / dur);
    out.push({
      start,
      stop: start + dur,
      title: slot === 0 ? seed.title : PROGRAMMES[Math.abs(slot * 7 + row.streamId) % PROGRAMMES.length],
      description: slot % 3 === 0 ? seed.description : null,
      // Some guides number their episodes; most programmes are not series.
      ...(Math.abs(slot) % 4 === 1
        ? { season: 1 + (row.streamId % 5), episode: 1 + (Math.abs(slot) % 12) }
        : {}),
    });
    if (out.length > 400) break;
  }
  return out;
}

/** The share of channels with guide data. Most of a real catalogue has none,
 *  so that is the default; `?guide=full` gives every channel a schedule, for
 *  pictures of the guide that show what it looks like with data in it. */
const GUIDE_SHARE =
  typeof location !== "undefined" && new URLSearchParams(location.search).get("guide") === "full"
    ? 1
    : 0.15;

function buildCatalog(): MockCatalog {
  const nowSec = Math.floor(Date.now() / 1000);
  const categories: CategoryDto[] = [];
  const rows: ChannelDto[] = [];
  const span = new Map<string, [number, number]>();

  // Skewed sizes: a handful of huge categories, a long tail of small ones —
  // the shape large catalogues have.
  const weights: number[] = [];
  let weightSum = 0;
  for (let i = 0; i < CATEGORY_TOTAL; i++) {
    const w = Math.pow(rand(), 2.2) + 0.02;
    weights.push(w);
    weightSum += w;
  }

  let streamId = 100000;
  let num = 1;
  let assigned = 0;

  for (let i = 0; i < CATEGORY_TOTAL; i++) {
    const country = COUNTRIES[i % COUNTRIES.length];
    const topic = TOPICS[Math.floor(i / COUNTRIES.length) % TOPICS.length];
    const dup = Math.floor(i / (COUNTRIES.length * TOPICS.length));
    const name = dup === 0 ? `${country} | ${topic}` : `${country} | ${topic} ${dup + 1}`;
    const id = `cat-${i}`;

    const last = i === CATEGORY_TOTAL - 1;
    const want = last
      ? CHANNEL_TOTAL - assigned
      : Math.max(1, Math.round((weights[i] / weightSum) * CHANNEL_TOTAL));
    const count = Math.max(1, Math.min(want, CHANNEL_TOTAL - assigned - (CATEGORY_TOTAL - 1 - i)));
    assigned += count;

    const start = rows.length;
    // A divider opens most categories, the way providers inject them.
    rows.push(separatorRow(streamId++, `###### ${topic} ######`, id));

    for (let c = 0; c < count; c++) {
      if (c > 0 && c % SEPARATOR_EVERY === 0) {
        rows.push(
          separatorRow(streamId++, `###### ${topic} — PART ${c / SEPARATOR_EVERY + 1} ######`, id),
        );
      }
      const network = pick(NETWORKS);
      const suffix = pick(SUFFIXES);
      const clean = `${network}${suffix}`;
      const full = `${country}: ${clean}`;
      const hasArchive = rand() < 0.01;
      const logoRoll = rand();
      rows.push({
        streamId: streamId++,
        name: full,
        cleanName: clean,
        num: num++,
        // Three states, all of which happen for real: a logo that loads, a
        // URL that 404s (very common in large catalogues), and no logo at all.
        logoUrl:
          logoRoll < 0.35
            ? inlineLogo(network)
            : logoRoll < 0.8
              ? `https://logo.invalid/${network.replace(/\s+/g, "")}.png`
              : null,
        categoryId: id,
        hasArchive,
        archiveDays: hasArchive ? (rand() < 0.5 ? 3 : 7) : null,
        isSeparator: false,
        quality: makeQuality(full, streamId - 1, nowSec),
        nowPlaying: rand() < GUIDE_SHARE ? makeProgramme(nowSec) : null,
      });
    }

    span.set(id, [start, rows.length - start]);
    categories.push({ id, name, channelCount: count });
  }

  // Over-the-air subchannels, numbered the way HDHomeRun and Tvheadend
  // number them: "5.1" held as 5001, so it sorts among the 5000s. Enough to
  // type a number with a dot in it.
  for (const r of rows) {
    if (r.num !== null && r.num >= 5001 && r.num <= 5003) r.numLabel = `5.${r.num - 5000}`;
  }

  // One guaranteed specimen of the lying-name case, near the top of the list.
  const liar = rows.find((r) => !r.isSeparator);
  if (liar) {
    liar.name = "CA: HARBOUR SPORTS 1 4K";
    liar.cleanName = "HARBOUR SPORTS 1 4K";
    liar.quality = makeQuality("CA: HARBOUR SPORTS 1 4K", liar.streamId, nowSec);
    liar.hasArchive = true;
    liar.archiveDays = 3;
    liar.nowPlaying = makeProgramme(nowSec);
  }

  return { categories, rows, span };
}

/** A tiny SVG wordmark as a data URI, so some rows really do show a logo. */
function inlineLogo(network: string): string {
  const initials = network
    .split(/[\s:+]+/)
    .map((w) => w[0])
    .join("")
    .slice(0, 3);
  const hue = (network.length * 47) % 360;
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="44" height="32">` +
    `<rect width="44" height="32" rx="4" fill="hsl(${hue} 45% 32%)"/>` +
    `<text x="22" y="21" font-family="sans-serif" font-size="13" font-weight="700" ` +
    `fill="#eef" text-anchor="middle">${initials}</text></svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

function separatorRow(streamId: number, name: string, categoryId: string): ChannelDto {
  return {
    streamId,
    name,
    cleanName: name.replace(/#/g, "").trim(),
    num: null,
    logoUrl: null,
    categoryId,
    hasArchive: false,
    archiveDays: null,
    isSeparator: true,
    quality: null,
    nowPlaying: null,
  };
}

let catalog: MockCatalog | null = null;

/** `?catalogue=empty` opens on the first-run mirror: no categories, no
 *  channels, never synced. The catalogue arrives when a sync's channels
 *  phase ends, part-way through the pass, as the real one does — which is
 *  what the lists have to notice without a restart. */
const flag = (re: RegExp) => typeof location !== "undefined" && re.test(location.search);

/** `?account=none` is a first launch: no account on file, so an empty
 *  catalogue too, and a sync that fails saying so. */
const NO_ACCOUNT = flag(/(^|[?&])account=none(&|$)/);
const MOCK_NO_ACCOUNT = "no account configured. Set one in Settings → Account.";

const START_EMPTY = NO_ACCOUNT || flag(/(^|[?&])catalogue=empty(&|$)/);
let catalogueLanded = !START_EMPTY;
const EMPTY_CATALOG: MockCatalog = { categories: [], rows: [], span: new Map() };

const getCatalog = (): MockCatalog =>
  catalogueLanded ? (catalog ??= buildCatalog()) : EMPTY_CATALOG;

// ------------------------------------------------------------- favourites

/** `?favourites=empty` starts with nothing, which is the ordinary first-run
 *  state and the one the pane has to say something sensible about. It is a
 *  query flag rather than a fixture toggle because it is the *default* in
 *  real life and the interesting case to look at. */
const EMPTY_FAVOURITES =
  typeof location !== "undefined" && /(^|[?&])favourites=empty(&|$)/.test(location.search);

/** The activity table, as the backend would have accumulated it: watch time
 *  against a stream id, and when it was last watched.
 *
 *  Deliberately not "the top N channels in order" — the backend ranks by watch
 *  time decayed against recency, and the mock has to be able to get that
 *  ordering wrong if the frontend ever starts sorting for itself. A binge
 *  months ago, a habit this week, and a documentary on once. */
interface MockActivity {
  watchSeconds: number;
  /** Seconds before now. */
  ago: number;
  openCount: number;
}

let activity = new Map<number, MockActivity>();
let activitySeeded = false;

function seedActivity(rows: ChannelDto[]) {
  activitySeeded = true;
  activity = new Map();
  if (EMPTY_FAVOURITES) return;
  const DAY = 86_400;
  const plan: MockActivity[] = [
    // Watched all week: modest hours, yesterday. Should lead.
    { watchSeconds: 9 * 3600 + 14 * 60, ago: 2 * 3600, openCount: 41 },
    // A weekend binge two months ago: far more hours, long decayed.
    { watchSeconds: 46 * 3600, ago: 58 * DAY, openCount: 12 },
    { watchSeconds: 4 * 3600 + 20 * 60, ago: 3 * DAY, openCount: 22 },
    { watchSeconds: 72 * 60, ago: 9 * DAY, openCount: 6 },
    { watchSeconds: 51 * 60, ago: 26 * 3600, openCount: 3 },
    { watchSeconds: 12 * 60, ago: 40 * 60, openCount: 2 },
    { watchSeconds: 47, ago: 5 * DAY, openCount: 1 },
  ];
  const playable = rows.filter((r) => !r.isSeparator);
  plan.forEach((a, i) => {
    // Spread across the catalogue rather than taking the first seven rows, so
    // the favourites are not simply the top of the channel list.
    const row = playable[Math.floor((i * 6131 + 733) % playable.length)];
    if (row) activity.set(row.streamId, a);
  });
}

function favouriteRows(limit: number): FavouriteDto[] {
  const c = getCatalog();
  if (!activitySeeded) seedActivity(c.rows);
  const now = Math.floor(Date.now() / 1000);
  const HALF_LIFE = 14 * 86_400;
  const byId = new Map(c.rows.map((r) => [r.streamId, r]));

  const scored: Array<{ score: number; f: FavouriteDto }> = [];
  for (const [streamId, a] of activity) {
    const channel = byId.get(streamId);
    // The same two exclusions the backend applies: a separator is not
    // playable and a deleted channel is not there.
    if (!channel || channel.isSeparator) continue;
    const lastWatched = now - a.ago;
    // Mirrors catalog::activity::score, so the mock cannot show an order the
    // backend would never produce.
    const score = a.watchSeconds * Math.pow(0.5, a.ago / HALF_LIFE);
    if (score <= 0) continue;
    scored.push({
      score,
      f: {
        channel,
        watchSeconds: a.watchSeconds,
        lastWatched,
        openCount: a.openCount,
      },
    });
  }
  scored.sort((x, y) => y.score - x.score || x.f.channel.streamId - y.f.channel.streamId);
  return scored.slice(0, limit).map((s) => s.f);
}

/** The channels watched most recently, newest first, whatever the watch time;
 *  mirrors catalog::activity::recent over the same activity map. */
function recentRows(limit: number): FavouriteDto[] {
  const c = getCatalog();
  if (!activitySeeded) seedActivity(c.rows);
  const now = Math.floor(Date.now() / 1000);
  const byId = new Map(c.rows.map((r) => [r.streamId, r]));
  const rows: Array<{ ago: number; f: FavouriteDto }> = [];
  for (const [streamId, a] of activity) {
    const channel = byId.get(streamId);
    if (!channel || channel.isSeparator) continue;
    rows.push({
      ago: a.ago,
      f: {
        channel,
        watchSeconds: a.watchSeconds,
        lastWatched: now - a.ago,
        openCount: a.openCount,
      },
    });
  }
  rows.sort((x, y) => x.ago - y.ago || x.f.channel.streamId - y.f.channel.streamId);
  return rows.slice(0, limit).map((r) => r.f);
}

// ----------------------------------------------------------------- the fake

const delay = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

type Listener<T> = (v: T) => void;

class Emitter<T> {
  private listeners = new Set<Listener<T>>();
  last: T;
  constructor(initial: T) {
    this.last = initial;
  }
  on(f: Listener<T>): () => void {
    this.listeners.add(f);
    return () => this.listeners.delete(f);
  }
  emit(v: T) {
    this.last = v;
    for (const f of this.listeners) f(v);
  }
}

const playerEmitter = new Emitter<PlayerStateDto>({
  state: "stopped",
  streamId: null,
  width: null,
  height: null,
  fps: null,
  codec: null,
  bufferedBack: 0,
  behindLive: 0,
  atLiveEdge: true,
  volume: 100,
  muted: false,
  compressor: false,
  error: null,
  liveJump: null,
  joiningLive: false,
  ending: null,
  reconnect: null,
  onDemand: null,
  finished: null,
});

const optionsEmitter = new Emitter<PlayerOptionsDto>({
  audio: [],
  subtitles: [],
  subtitlesVisible: false,
  aspect: "auto",
  deinterlace: "auto",
});

const track = (id: number, t: Partial<TrackDto>): TrackDto => ({
  id,
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

/** Tracks for a stream, so the picture menu has something to show. Three
 *  shapes by stream id: the common one (a single audio track, no
 *  subtitles, so no track choice is offered at all), a bilingual broadcast
 *  with DVB subtitles, and one with audio description and a track with no
 *  language. */
function mockTracks(streamId: number): Pick<PlayerOptionsDto, "audio" | "subtitles"> {
  switch (streamId % 3) {
    case 1:
      return {
        audio: [
          track(1, { lang: "eng", codec: "ac3", channels: 6, default: true, selected: true }),
          track(2, { lang: "fre", codec: "aac", channels: 2 }),
          track(3, { lang: "spa", codec: "mp2", channels: 2, title: "Commentary" }),
        ],
        subtitles: [
          track(1, { lang: "eng", codec: "dvb_subtitle" }),
          track(2, { lang: "eng", codec: "dvb_teletext", hearingImpaired: true }),
          track(3, { lang: "fre", codec: "dvb_subtitle" }),
        ],
      };
    case 2:
      return {
        audio: [
          track(1, { lang: "deu", codec: "aac", channels: 2, default: true, selected: true }),
          track(2, { lang: "deu", codec: "aac", channels: 2, visualImpaired: true }),
          track(3, { codec: "eac3", channels: 6 }),
        ],
        subtitles: [track(1, { lang: "deu", codec: "dvb_teletext" })],
      };
    default:
      return { audio: [track(1, { lang: "eng", codec: "aac", channels: 2, selected: true })], subtitles: [] };
  }
}

/** Subtitles on or off, and in what language: the app's standing choice,
 *  as the backend's player keeps it. Set from the settings, and at once by a
 *  choice of subtitles; applied to every stream opened. */
let mockSubtitles: SubtitlePreference = { subtitlesOn: false, subtitleLanguage: null };

/** A stream's tracks with the standing subtitle choice applied, as the
 *  backend applies it: a track chosen whenever there is one, shown when
 *  subtitles are on and hidden when they are off. */
function withStandingSubtitles(
  t: Pick<PlayerOptionsDto, "audio" | "subtitles">,
): Pick<PlayerOptionsDto, "audio" | "subtitles" | "subtitlesVisible"> {
  const choice = openChoice(mockSubtitles, t.subtitles, true);
  if (choice === undefined) return { ...t, subtitlesVisible: false };
  return {
    ...t,
    subtitles: t.subtitles.map((s) => ({ ...s, selected: s.id === choice.id })),
    subtitlesVisible: choice.visible,
  };
}

function patchOptions(p: Partial<PlayerOptionsDto>): PlayerOptionsDto {
  optionsEmitter.emit({ ...optionsEmitter.last, ...p });
  return optionsEmitter.last;
}

const syncEmitter = new Emitter<SyncStatusDto>({
  phase: NO_ACCOUNT ? "failed" : "idle",
  message: NO_ACCOUNT ? MOCK_NO_ACCOUNT : "",
  channelsTotal: START_EMPTY ? 0 : CHANNEL_TOTAL,
  lastSynced: START_EMPTY ? null : Math.floor(Date.now() / 1000) - 3600,
});

let mockDropped = 0;

/** Samples in the mock's synthetic history. Matches HISTORY_LENGTH in
 *  src/logic/stats.ts, so the graphs are drawn full. */
const MOCK_HISTORY = 120;
let mockAccount: AccountDto = {
  host: "http://tv.example.com:8080",
  username: "demo",
  hasPassword: true,
  source: "zapper",
  path: "~/.config/zapper/account.json",
  kind: "xtream",
  // A provider-style playlist on file, so the redacted-field path is what
  // the mock shows when switched to M3U.
  playlist: "http://tv.example.com:8080/get.php?username=<redacted>&password=<redacted>&type=m3u_plus",
  playlistRedacted: true,
  epgUrls: [],
  epgRedacted: false,
  userAgent: "",
  hdhomerunDevice: "",
  hdhomerunEpgUrls: [],
  hdhomerunEpgRedacted: false,
  tvheadendUrl: "",
  tvheadendUsername: "",
  tvheadendHasPassword: false,
};
if (NO_ACCOUNT) {
  mockAccount = {
    ...mockAccount,
    host: "",
    username: "",
    hasPassword: false,
    source: "none",
    playlist: "",
    playlistRedacted: false,
  };
}

/** The mock's redaction, so a saved secret-bearing URL comes back the way
 *  the real backend would send it. */
const mockRedact = (u: string) => u.replace(/((?:username|password|token|key)=)[^&]*/gi, "$1<redacted>");
let bufferTimer: number | undefined;

/** Where the standing subtitle choice is kept across a reload of the tab. */
const MOCK_SUBTITLES_KEY = "zapper.mock.subtitles";

/** Settings survive for the life of the page, so a save can be read back and
 *  a reload of the pane shows what was stored rather than the defaults. Not
 *  the contract's defaults on purpose: if the UI ever ignored what the backend
 *  returned and painted its own defaults, these values would show it. */
let storedSettings: SettingsDto = {
  language: "auto",
  rewindMb: 546,
  hardwareDecode: true,
  toneMapHdr: false,
  guideHours: 3,
  syncIntervalHours: 12,
  subtitle: {
    font: "Sans",
    size: 42,
    color: "#ffffff",
    backdrop: "background",
    backdropOpacity: 0.25,
    outlineSize: 2,
    shadowOffset: 0,
  },
  checkForUpdates: true,
  mediaControls: false,
  timeFormat: "auto",
  liveStart: "catchUp",
  simultaneousStreams: 1,
  subtitlesOn: false,
  subtitleLanguage: null,
  // The standing subtitle choice outlives a reload of the tab, as the
  // app's outlives a restart: a driver checks that it does.
  ...storedSubtitles(),
};
mockSubtitles = {
  subtitlesOn: storedSettings.subtitlesOn,
  subtitleLanguage: storedSettings.subtitleLanguage,
};

function storedSubtitles(): Partial<SubtitlePreference> {
  try {
    const raw = sessionStorage.getItem(MOCK_SUBTITLES_KEY);
    const v = raw ? (JSON.parse(raw) as Partial<SubtitlePreference>) : {};
    return {
      ...(typeof v.subtitlesOn === "boolean" ? { subtitlesOn: v.subtitlesOn } : {}),
      ...(v.subtitleLanguage === null || typeof v.subtitleLanguage === "string"
        ? { subtitleLanguage: v.subtitleLanguage }
        : {}),
    };
  } catch {
    return {};
  }
}

// ----------------------------------------------------------------- updates

/** `?updates=` picks what the fake GitHub says, so every state of the
 *  update dialog can be looked at without a release:
 *
 *    available     a .deb install with 0.2.0 waiting (the default)
 *    source        a build from source: only the release page is offered
 *    arch          as available, for an Arch package install
 *    manual        the install "fails" and hands back the command to run
 *    current       nothing newer
 *    unconfigured  the placeholder repository: no check ever happens
 *    offline       every check fails
 *    recording     a Windows copy whose recording service is recording until
 *                  ten minutes from now: Install asks first */
const UPDATES_MODE =
  (typeof location !== "undefined" &&
    new URLSearchParams(location.search).get("updates")) ||
  "available";

const MOCK_RELEASE: UpdateReleaseDto = {
  version: "0.2.0",
  name: "zapper 0.2.0",
  notes:
    "### Added\n- Check for updates, and install them from inside the app.\n\n### Fixed\n- The guide no longer jumps when a programme ends.",
  url: "https://github.com/francislavoie/zapper/releases/tag/v0.2.0",
  prerelease: false,
  publishedAt: "2026-10-30T12:00:00Z",
  asset:
    UPDATES_MODE === "source"
      ? null
      : UPDATES_MODE === "arch"
        ? { name: "zapper-0.2.0-1-x86_64.pkg.tar.zst", size: 6_300_000 }
        : { name: "zapper_0.2.0_amd64.deb", size: 9_400_000 },
};

const updateEmitter = new Emitter<UpdateStatusDto>({
  current: "0.1.0",
  kind: (UPDATES_MODE === "source" ? "none" : UPDATES_MODE === "arch" ? "arch" : "deb") as InstallKind,
  lastChecked: null,
  state: UPDATES_MODE === "unconfigured" ? { state: "notConfigured" } : { state: "idle" },
});

let updateTimer: number | undefined;

function setUpdate(p: Partial<UpdateStatusDto>) {
  updateEmitter.emit({ ...updateEmitter.last, ...p });
  // Exposed so a driver can assert on what the backend thinks, not only on
  // what the dialog drew.
  (globalThis as Record<string, unknown>).__mockUpdate = updateEmitter.last;
}

function patchPlayer(p: Partial<PlayerStateDto>) {
  playerEmitter.emit({ ...playerEmitter.last, ...p });
}

/** A driver's way to end the current stream as a provider would:
 *  `__mockEnd("dropped")`, `__mockEnd("refused", 509, true)`. The real
 *  backend's reasons are in `ipc::ending`. */
(globalThis as Record<string, unknown>).__mockEnd = (
  kind: NonNullable<PlayerStateDto["ending"]>["kind"],
  status: number | null = null,
  limit = false,
) => {
  patchPlayer({
    state: "failed",
    error: "the stream ended",
    ending: { kind, status, limit, slots: kind === "inUse" ? { max: 1, active: 1 } : null, clipSecs: null },
    reconnect: null,
    bufferedBack: 0,
    behindLive: 0,
  });
};

/** A driver's way to find the current channel off air, as the backend does
 *  when the provider sends a placeholder clip (`ending::off_air` in Rust):
 *  `__mockOffAir()`, or `__mockOffAir(45)` for a clip under a minute. The
 *  channel's record says so too, as the backend's ledger would. */
(globalThis as Record<string, unknown>).__mockOffAir = (clipSecs = 600) => {
  const streamId = playerEmitter.last.streamId;
  patchPlayer({
    state: "failed",
    error: "the provider sent a placeholder instead of the channel",
    ending: { kind: "offAir", status: null, limit: false, slots: null, clipSecs },
    reconnect: null,
    joiningLive: false,
    bufferedBack: 0,
    behindLive: 0,
  });
  if (streamId !== null) {
    remember(streamId, (q) => ({
      failCount: q.failCount + 1,
      failedAt: Math.floor(Date.now() / 1000),
      failReason: "off air",
    }));
  }
};

/** The guide id the mock gives a channel: its country and network, without
 *  the feed's suffix, so the HD, 4K and regional copies of a channel share
 *  one, as a provider's guide would have them. */
function mockGuideId(c: ChannelDto): string | null {
  if (c.isSeparator) return null;
  return c.name.replace(/ (HD|FHD|4K|RAW|EAST|WEST)$/, "");
}

/** A driver's way to show the backend reconnecting after a network failure:
 *  `__mockReconnect(2)`, or `__mockReconnect(1, true)` while it asks the
 *  account for a free connection. */
(globalThis as Record<string, unknown>).__mockReconnect = (attempt = 1, checking = false) => {
  patchPlayer({
    state: "failed",
    error: "the stream could not be opened",
    ending: { kind: "network", status: null, limit: false, slots: null, clipSecs: null },
    reconnect: { attempt, of: 4, checking },
    bufferedBack: 0,
    behindLive: 0,
  });
};

/** The buffer only grows while something is playing; the scrubber's archive
 *  seam has to move for its geometry to be worth checking. */
function startBufferClock() {
  if (bufferTimer !== undefined) return;
  bufferTimer = window.setInterval(() => {
    const p = playerEmitter.last;
    if (p.state !== "playing") return;
    patchPlayer({ bufferedBack: Math.min(240, p.bufferedBack + 1) });
  }, 1000);
}

/** What playing a channel added to its record, as the backend reports it;
 *  see `remember` in ipc/player_cmds.rs. */
const qualityEmitter = new Emitter<ChannelQualityDto>({ streamId: -1, quality: null });

/** The backend's ledger, in miniature: the row's record is updated in place,
 *  so pages fetched later carry it, and the change is announced for the
 *  pages already on screen. */
function remember(streamId: number, change: (q: QualityDto) => Partial<QualityDto>) {
  const rows = getCatalog().rows;
  const at = rows.findIndex((r) => r.streamId === streamId);
  if (at < 0) return;
  // A new row rather than the old one changed: the pages on screen hold the
  // old one, as the backend's would hold what it sent, so the news differs
  // from it and the row redraws.
  const row = { ...rows[at] };
  rows[at] = row;
  const q: QualityDto = row.quality ?? {
    codec: null,
    width: null,
    height: null,
    fps: null,
    interlaced: null,
    isHdr: false,
    audioCodec: null,
    audioChannels: null,
    measuredAt: null,
    okCount: 0,
    failCount: 0,
    failedAt: null,
    failReason: null,
  };
  row.quality = { ...q, ...change(q) };
  qualityEmitter.emit({ streamId, quality: row.quality });
}

/** Channels the mock has already failed once this session; see makeQuality. */
const failedOnce = new Set<number>();
/** "Start channels at live", as last applied. */
let mockLiveStart: SettingsDto["liveStart"] = "catchUp";

/** Counts the mock's automatic catch-ups, like the backend's `seq`. */
let mockJumps = 0;

/** Bumped on every open, so a catch-up timer from the last stream cannot
 *  land on the next one. */
let openSeq = 0;

async function openStream(
  streamId: number,
  quality: Partial<PlayerStateDto>,
  { live = true, fail = false }: { live?: boolean; fail?: boolean } = {},
) {
  const mine = ++openSeq;
  patchPlayer({
    state: "opening",
    streamId,
    error: null,
    ending: null,
    bufferedBack: 0,
    behindLive: 0,
    liveJump: null,
    joiningLive: false,
    reconnect: null,
    onDemand: null,
    finished: null,
  });
  patchOptions({ audio: [], subtitles: [] });
  await delay(700);
  if (openSeq !== mine) return;
  if (fail) {
    const reason = "refused by the provider (HTTP 403)";
    patchPlayer({
      state: "failed",
      error: "the stream could not be opened",
      ending: { kind: "refused", status: 403, limit: false, slots: null, clipSecs: null },
    });
    remember(streamId, (q) => ({
      failCount: q.failCount + 1,
      failedAt: Math.floor(Date.now() / 1000),
      failReason: reason,
    }));
    return;
  }
  if (live) remember(streamId, (q) => ({ okCount: q.okCount + 1, failedAt: null, failReason: null }));
  if (mockLiveStart === "wait") {
    // The backend's Join: the first frame held while the burst arrives,
    // then started at live, with nothing to jump.
    patchPlayer({ ...quality, joiningLive: true });
    patchOptions(withStandingSubtitles(mockTracks(streamId)));
    await delay(1500);
    if (openSeq !== mine || playerEmitter.last.state !== "opening") return;
    patchPlayer({ state: "playing", joiningLive: false, behindLive: 4, bufferedBack: 4, atLiveEdge: true });
    startBufferClock();
    return;
  }
  // The connect burst: playback starts well behind live, as the real thing
  // does, and the backend's Settle brings it up once the burst is over —
  // reported as a jump, which is what the picture's corner announces.
  const behind = 14 + Math.round(Math.random() * 100) / 10;
  patchPlayer({ state: "playing", ...quality, behindLive: behind, bufferedBack: behind, atLiveEdge: false });
  patchOptions(withStandingSubtitles(mockTracks(streamId)));
  startBufferClock();
  if (live) {
    // Measured a few seconds in, as the backend does.
    window.setTimeout(() => {
      if (openSeq !== mine || playerEmitter.last.state !== "playing") return;
      const p = playerEmitter.last;
      remember(streamId, () => ({
        width: p.width,
        height: p.height,
        fps: p.fps,
        codec: p.codec,
        measuredAt: Math.floor(Date.now() / 1000),
      }));
    }, 5000);
  }
  // Staying put: behind it started, and behind it stays.
  if (mockLiveStart === "stay") return;
  await delay(2500);
  if (openSeq !== mine || playerEmitter.last.state !== "playing") return;
  const cushion = 4;
  patchPlayer({
    behindLive: cushion,
    atLiveEdge: true,
    liveJump: { seconds: Math.round((behind - cushion) * 10) / 10, seq: ++mockJumps },
  });
}

// ------------------------------------------------------------- recording

/** The recording service, faked: installed and running, with a recording in
 *  progress, a couple scheduled on channels the guide shows, and one each of
 *  done and failed, so every state of the list and the guide's dots can be
 *  seen. `?recording=off` makes it not answer, as when it is not installed. */
const RECORDING_OFF =
  typeof location !== "undefined" &&
  new URLSearchParams(location.search).get("recording") === "off";
/** `?service=windows`: the service as Windows installs it, a sign-in task
 *  (or, switched to, the service that starts at boot). */
const SERVICE_WINDOWS =
  typeof location !== "undefined" &&
  new URLSearchParams(location.search).get("service") === "windows";

let mockRecordings: RecordingDto[] | null = null;
let mockRecordingId = 100;
const mockRecordingSettings: RecordingSettingsDto = {
  outputDir: "/home/viewer/Videos/zapper",
  naming: "mediaServer",
  namingPattern: "",
  padBefore: 60,
  padAfter: 300,
  clockTrack: true,
  whileRecording: "hidden",
  showWhileRecording: false,
  strmHost: "",
  defaultOutputDir: "/home/viewer/Videos/zapper",
};

/** Where the service would point `.strm` files (recording::strm::base_url):
 *  the host chosen, else the network address while network access is on,
 *  else this computer. */
function mockStrmBase(): string {
  const host = mockRecordingSettings.strmHost?.trim() || (mockNetwork.enabled ? "192.168.1.20" : "127.0.0.1");
  const local = host === "127.0.0.1" || host === "localhost" || host === "::1" || host === "[::1]";
  const scheme = !local && mockNetwork.enabled && mockNetwork.https ? "https" : "http";
  const shown = host.includes(":") && !host.startsWith("[") ? `[${host}]` : host;
  return `${scheme}://${shown}:${mockNetwork.port}`;
}
let mockService: ServiceStatusDto = SERVICE_WINDOWS
  ? {
      supported: true,
      kind: "a scheduled task that starts it when you sign in",
      installed: true,
      state: "running",
      linger: false,
      canLinger: true,
      bootService: true,
      binary: "C:\\Users\\viewer\\AppData\\Local\\zapper\\zapper-server.exe",
      version: "0.1.0",
      address: "http://127.0.0.1:8796",
      versionMismatch: false,
    }
  : {
      supported: true,
      kind: "a systemd user service",
      installed: true,
      state: "active",
      linger: false,
      canLinger: true,
      bootService: false,
      binary: "/usr/bin/zapper-server",
      version: "0.1.0",
      address: "http://127.0.0.1:8796",
      versionMismatch: false,
    };

/** What Windows calls the service, task or boot-time service. */
function mockWindowsKind(boot: boolean): Partial<ServiceStatusDto> {
  if (!mockService.bootService) return {};
  return {
    kind: boot
      ? "a Windows service that starts it when the computer starts"
      : "a scheduled task that starts it when you sign in",
  };
}

function mockRecording(
  ch: ChannelDto,
  p: ProgrammeDto,
  state: RecordingDto["state"],
  extra: Partial<RecordingDto> = {},
): RecordingDto {
  const s = mockRecordingSettings;
  const at = new Date(p.start * 1000);
  const { folders } = place(
    patternFor(s.naming, s.namingPattern),
    { title: p.title, channel: ch.cleanName },
    {
      year: at.getFullYear(),
      month: at.getMonth() + 1,
      day: at.getDate(),
      hour: at.getHours(),
      minute: at.getMinutes(),
    },
  );
  const folder = [s.outputDir, ...folders].join("/");
  return {
    id: mockRecordingId++,
    streamId: ch.streamId,
    backupStreamId: null,
    channelName: ch.cleanName,
    title: p.title,
    start: p.start,
    stop: p.stop,
    padBefore: mockRecordingSettings.padBefore,
    padAfter: mockRecordingSettings.padAfter,
    state,
    parts: [],
    notes: [],
    folder,
    ...extra,
  };
}

function seedRecordings(): RecordingDto[] {
  const now = Math.floor(Date.now() / 1000);
  const withGuide = getCatalog().rows.filter((r) => r.nowPlaying && !r.isSeparator).slice(0, 3);
  const out: RecordingDto[] = [];
  const [a, b, c] = withGuide;
  if (a) {
    const [on, next] = schedule(a, now, now + 6 * 3600);
    if (on) {
      out.push(
        mockRecording(a, on, "recording", {
          parts: [
            { path: `/home/viewer/Videos/zapper/${on.title}/.part1.ts`, streamId: a.streamId, bytes: 412e6, started: on.start - 60, ended: null },
          ],
        }),
      );
    }
    if (next) {
      const spare = getCatalog().rows.find(
        (r) => r.categoryId === a.categoryId && r.streamId !== a.streamId && !r.isSeparator,
      );
      out.push(
        mockRecording(a, next, "scheduled", spare ? { backupStreamId: spare.streamId, backupName: spare.cleanName } : {}),
      );
    }
  }
  if (b) {
    const later = schedule(b, now + 3600, now + 8 * 3600)[1];
    if (later) {
      // On an account of one stream, as the mock's is: what the service
      // would say of a programme following another channel's.
      const before = out.find((r) => r.state === "scheduled");
      out.push(
        mockRecording(b, later, "scheduled", {
          conflicts: before
            ? [{ title: later.title, with: before.title, kind: "lateStart", secs: 60, streams: 1 }]
            : undefined,
        }),
      );
    }
  }
  if (c) {
    const yesterday = { start: now - 86_400, stop: now - 86_400 + 3600, title: "The Late Film", description: null };
    out.push(
      mockRecording(c, yesterday, "done", {
        parts: [
          { path: "/home/viewer/Videos/zapper/The Late Film/The Late Film - part1.mkv", streamId: c.streamId, bytes: 1.21e9, started: yesterday.start - 60, ended: yesterday.start + 1500 },
          { path: "/home/viewer/Videos/zapper/The Late Film/The Late Film - part2.mkv", streamId: c.streamId + 1, bytes: 1.4e9, started: yesterday.start + 1500, ended: yesterday.stop + 300 },
        ],
        notes: ["switched from the main feed to the backup: no data for 10 s"],
      }),
    );
    const missed = { start: now - 2 * 86_400, stop: now - 2 * 86_400 + 1800, title: "Morning News", description: null };
    out.push(mockRecording(c, missed, "failed", { error: "missed: the recording service was not running" }));
  }
  return out;
}

/** Network access, faked: off with no password, as a new service is.
 *  `?network=on` starts it on, with a password and two devices signed in, so
 *  the whole box can be seen at once. Port 8080 stands for one another
 *  program holds. */
const NETWORK_ON =
  typeof location !== "undefined" &&
  new URLSearchParams(location.search).get("network") === "on";
const MOCK_FINGERPRINT =
  "3F:9A:1C:72:E4:08:B5:6D:21:CE:90:47:AB:13:F8:62:5D:0E:C9:34:87:2B:D1:6A:F0:45:9E:B8:13:7C:2A:D5";

function mockNetworkSeed(): NetworkDto {
  const now = Math.floor(Date.now() / 1000);
  return {
    enabled: NETWORK_ON,
    port: 8796,
    https: true,
    passwordSet: NETWORK_ON,
    fingerprint: NETWORK_ON ? MOCK_FINGERPRINT : undefined,
    addresses: [],
    devices: NETWORK_ON
      ? [
          {
            id: "5be1c0a2f4d93e70",
            name: "Firefox on Android",
            userAgent: "Mozilla/5.0 (Android 14; Mobile; rv:131.0) Gecko/131.0 Firefox/131.0",
            created: now - 12 * 86_400,
            lastUsed: now - 540,
            address: "192.168.1.31",
          },
          {
            id: "91a7d3e0c6b2f845",
            name: "Living room TV",
            userAgent: "Mozilla/5.0 (X11; Linux armv7l) AppleWebKit/537.36 Chrome/120.0.0.0 Safari/537.36",
            created: now - 40 * 86_400,
            lastUsed: now - 3 * 86_400,
            address: "192.168.1.44",
          },
        ]
      : [],
  };
}

let mockNetwork: NetworkDto = mockNetworkSeed();

function networkView(): NetworkDto {
  const scheme = mockNetwork.https ? "https" : "http";
  return {
    ...mockNetwork,
    addresses: [
      `${scheme}://192.168.1.20:${mockNetwork.port}`,
      `${scheme}://[fd00::1a2b:3c4d]:${mockNetwork.port}`,
    ],
    devices: mockNetwork.devices.map((d) => ({ ...d })),
  };
}

/** `?recording=busy`: two recordings at once, one of them waiting for a
 *  connection on its backup channel with a conflict ahead, and a third
 *  being saved — the title bar's indicator and Recordings with everything
 *  they can say. */
const RECORDING_BUSY =
  typeof location !== "undefined" &&
  new URLSearchParams(location.search).get("recording") === "busy";

function busyRecordings(out: RecordingDto[]): void {
  const now = Math.floor(Date.now() / 1000);
  const rows = getCatalog().rows.filter((r) => r.nowPlaying && !r.isSeparator);
  const [b, c, d] = rows.slice(3, 6);
  if (b) {
    const on = schedule(b, now, now + 3600)[0];
    if (on)
      out.push(
        mockRecording(b, on, "recording", {
          backupStreamId: c?.streamId ?? null,
          backupName: c?.cleanName,
          waiting: "waiting for a free connection (1 of 1 in use)",
          parts: [
            { path: "/home/viewer/Videos/zapper/x/.part1.ts", streamId: b.streamId, bytes: 80e6, started: on.start - 60, ended: on.start + 300 },
            { path: "/home/viewer/Videos/zapper/x/.part2.ts", streamId: c?.streamId ?? b.streamId, bytes: 30e6, started: on.start + 300, ended: null },
          ],
          conflicts: [{ title: on.title, with: out[0]?.title ?? "Film", kind: "cutEnd", secs: 300, streams: 1 }],
        }),
      );
  }
  if (d) {
    const was = { start: now - 3600, stop: now - 60, title: "Evening Concert", description: null };
    out.push(mockRecording(d, was, "finishing", { parts: [{ path: "/home/viewer/Videos/zapper/x/.part1.ts", streamId: d.streamId, bytes: 3.2e9, started: was.start - 60, ended: now - 30 }] }));
  }
}

function recordingsOrRefuse(): RecordingDto[] {
  if (RECORDING_OFF || !mockService.installed) throw "The recording service is not running.";
  if (!mockRecordings) {
    mockRecordings = seedRecordings();
    if (RECORDING_BUSY) busyRecordings(mockRecordings);
  }
  return mockRecordings;
}

const mockRecordingApi = {
  recordings: async (): Promise<RecordingDto[]> => {
    await delay(30);
    return recordingsOrRefuse().map((r) => ({ ...r }));
  },
  recordProgramme: async (req: ScheduleRequest): Promise<RecordingDto> => {
    await delay(60);
    const list = recordingsOrRefuse();
    const same = list.find((r) => r.streamId === req.streamId && r.start === req.start);
    if (same) return same;
    const ch = getCatalog().rows.find((r) => r.streamId === req.streamId);
    if (!ch) throw "no such channel";
    const now = Math.floor(Date.now() / 1000);
    const rec = mockRecording(
      ch,
      { start: req.start, stop: req.stop, title: req.title, description: null },
      req.start - mockRecordingSettings.padBefore <= now ? "recording" : "scheduled",
    );
    list.push(rec);
    return rec;
  },
  // The service plays the schedule through its rules (src-tauri/src/
  // recording/sharing.rs); this is a rough likeness on one stream: two
  // channels whose windows overlap clash, the earlier one keeping its
  // programme and the later one losing what overlaps it.
  previewRecording: async (req: ScheduleRequest): Promise<ConflictDto[]> => {
    await delay(40);
    const s = mockRecordingSettings;
    const asked = { title: req.title, start: req.start, stop: req.stop, padBefore: s.padBefore, padAfter: s.padAfter };
    const out: ConflictDto[] = [];
    for (const r of recordingsOrRefuse()) {
      if ((r.state !== "scheduled" && r.state !== "recording") || r.streamId === req.streamId) continue;
      if (r.stop + r.padAfter <= asked.start - asked.padBefore || asked.stop + asked.padAfter <= r.start - r.padBefore)
        continue;
      const [first, next] = r.start <= asked.start ? [r, asked] : [asked, r];
      const nextFrom = next.start - next.padBefore;
      const other = (x: typeof r | typeof asked) => x !== asked;
      if (first.stop <= next.start) {
        out.push({ title: first.title, with: next.title, kind: "cutEnd", secs: first.stop + first.padAfter - Math.max(nextFrom, first.stop), streams: 1, other: other(first) });
        if (first.stop > nextFrom)
          out.push({ title: next.title, with: first.title, kind: "lateStart", secs: first.stop - nextFrom, streams: 1, other: other(next) });
      } else if (next.stop <= first.stop) {
        out.push({ title: next.title, with: first.title, kind: "missed", secs: 0, streams: 1, other: other(next) });
      } else {
        out.push({ title: next.title, with: first.title, kind: "lostStart", secs: first.stop - next.start, streams: 1, other: other(next) });
      }
    }
    return out;
  },
  // The service ranks channels on the same guide channel, then the same
  // programme elsewhere (src-tauri/src/recording/backup.rs). The mock has no
  // guide channels, so its neighbours in the category stand in.
  recordingBackups: async (id: number): Promise<BackupsDto> => {
    await delay(50);
    const r = recordingsOrRefuse().find((x) => x.id === id);
    if (!r) throw "no such recording";
    const rows = getCatalog().rows;
    const main = rows.find((c) => c.streamId === r.streamId);
    const candidates = rows
      .filter((c) => !c.isSeparator && c.streamId !== r.streamId && c.categoryId === main?.categoryId)
      .slice(0, 6)
      .map((c, i) => ({
        streamId: c.streamId,
        name: c.cleanName || c.name,
        num: c.num ?? undefined,
        reason: (i < 4 ? "guide" : "programme") as BackupDto["reason"],
        height: c.quality?.height ?? undefined,
        failing: c.quality?.failedAt != null ? true : undefined,
      }));
    return { current: r.backupStreamId ?? undefined, candidates };
  },
  setRecordingBackup: async (id: number, streamId: number | null): Promise<RecordingDto> => {
    await delay(50);
    const r = recordingsOrRefuse().find((x) => x.id === id);
    if (!r) throw "no such recording";
    if (r.state !== "scheduled" && r.state !== "recording") throw "that recording has finished";
    if (streamId === r.streamId) throw "the backup must be another channel than the one recorded";
    if (streamId == null) {
      r.backupStreamId = null;
      delete r.backupName;
    } else {
      const ch = getCatalog().rows.find((c) => c.streamId === streamId && !c.isSeparator);
      if (!ch) throw `there is no channel ${streamId} to use as a backup`;
      r.backupStreamId = streamId;
      r.backupName = ch.cleanName || ch.name;
    }
    return { ...r };
  },
  cancelRecording: async (id: number) => {
    await delay(40);
    const list = recordingsOrRefuse();
    const r = list.find((x) => x.id === id);
    if (!r) throw "no such recording";
    if (r.state === "scheduled") mockRecordings = list.filter((x) => x.id !== id);
    else if (r.state === "recording") {
      // Stopped, it is saved into its file first, as the service does: a
      // few seconds here, minutes for a long recording there.
      r.state = "finishing";
      r.notes = [...r.notes, "stopped before its end"];
      setTimeout(() => {
        if (r.state === "finishing") r.state = "done";
      }, 6_000);
    }
  },
  deleteRecording: async (id: number, _files: boolean) => {
    await delay(40);
    mockRecordings = recordingsOrRefuse().filter((x) => x.id !== id);
  },
  recordingStatus: async (): Promise<RecordingStatusDto> => {
    const list = recordingsOrRefuse();
    const active = list
      .filter((r) => r.state === "recording" || r.state === "finishing")
      .map((r) => ({
        id: r.id,
        title: r.title,
        streamId: r.streamId,
        bytes: r.parts[0]?.bytes ?? 0,
        since: r.start,
        until: r.stop + r.padAfter,
        waiting: r.waiting,
        finishing: r.state === "finishing" ? true : undefined,
      }));
    return {
      version: "0.1.0",
      apiVersion: 1,
      active,
      outputDir: mockRecordingSettings.outputDir,
      diskFree: 412e9,
      diskTotal: 1e12,
      now: Math.floor(Date.now() / 1000),
      idle: active.length === 0,
      idleAt: active.length ? Math.max(...active.map((a) => a.until)) : undefined,
    };
  },
  recordingSettings: async (): Promise<RecordingSettingsDto> => {
    recordingsOrRefuse();
    return { ...mockRecordingSettings, strmBase: mockStrmBase() };
  },
  setRecordingSettings: async (next: RecordingSettingsDto): Promise<RecordingSettingsDto> => {
    recordingsOrRefuse();
    const outputDir = next.outputDir.trim() || mockRecordingSettings.defaultOutputDir;
    if (!outputDir.startsWith("/")) throw "the output folder must be a full path";
    const namingPattern = (next.namingPattern ?? "").trim();
    if (next.naming === "custom" || namingPattern) {
      const why = validatePattern(namingPattern);
      if (why) throw `file name pattern: ${why}`;
    }
    const strmHost = (next.strmHost ?? "").trim();
    if (/[/@?#\s]|:\d+$/.test(strmHost) && !strmHost.includes("::"))
      throw `“${strmHost}” is not a host name or address for .strm files (just the host: no port, no http://)`;
    const whileRecording = next.whileRecording ?? mockRecordingSettings.whileRecording;
    Object.assign(mockRecordingSettings, next, {
      outputDir,
      namingPattern,
      whileRecording,
      showWhileRecording: whileRecording === "visible",
      strmHost,
      defaultOutputDir: mockRecordingSettings.defaultOutputDir,
    });
    return { ...mockRecordingSettings, strmBase: mockStrmBase() };
  },
  recordingNetwork: async (): Promise<NetworkDto> => {
    recordingsOrRefuse();
    await delay(30);
    return networkView();
  },
  setRecordingNetwork: async (next: NetworkSettingsDto): Promise<NetworkDto> => {
    recordingsOrRefuse();
    await delay(150);
    if (parsePort(String(next.port)) === null) throw "the port must be between 1024 and 65535";
    if (next.enabled && !mockNetwork.passwordSet)
      throw "set a sign-in password before turning on network access";
    if (next.port === 8080) throw "port 8080 is in use by another program";
    mockNetwork = {
      ...mockNetwork,
      ...next,
      error: undefined,
      fingerprint:
        mockNetwork.fingerprint ?? (next.enabled && next.https ? MOCK_FINGERPRINT : undefined),
    };
    return networkView();
  },
  setRecordingPassword: async (password: string): Promise<NetworkDto> => {
    recordingsOrRefuse();
    await delay(250);
    const why = passwordProblem(password, password);
    if (why) throw why;
    // A new password signs every device out, as the service does.
    mockNetwork = { ...mockNetwork, passwordSet: true, devices: [] };
    return networkView();
  },
  revokeRecordingDevice: async (id: string): Promise<NetworkDto> => {
    recordingsOrRefuse();
    await delay(40);
    if (!mockNetwork.devices.some((d) => d.id === id)) throw "no such device";
    mockNetwork = { ...mockNetwork, devices: mockNetwork.devices.filter((d) => d.id !== id) };
    return networkView();
  },
  recordingService: async (): Promise<ServiceStatusDto> => {
    await delay(30);
    return RECORDING_OFF
      ? { ...mockService, installed: false, state: undefined, version: undefined }
      : { ...mockService };
  },
  installRecordingService: async (linger: boolean): Promise<ServiceStatusDto> => {
    await delay(300);
    mockService = {
      ...mockService,
      installed: true,
      state: SERVICE_WINDOWS ? "running" : "active",
      linger: mockService.canLinger && linger,
      ...mockWindowsKind(linger),
      version: "0.1.0",
    };
    return { ...mockService };
  },
  removeRecordingService: async (): Promise<ServiceStatusDto> => {
    await delay(200);
    mockService = { ...mockService, installed: false, state: undefined, version: undefined };
    return { ...mockService };
  },
  setRecordingLinger: async (on: boolean): Promise<ServiceStatusDto> => {
    mockService = { ...mockService, linger: on, ...mockWindowsKind(on) };
    return { ...mockService };
  },
  openRecordingFolder: async (id: number) => {
    (globalThis as Record<string, unknown>).__mockOpenedFolder = id;
  },
};

/** The movie catalogue and what plays one, which share the player below. */
const mockMovies = createMockMovies({
  state: () => playerEmitter.last,
  patch: patchPlayer,
  open: () => ++openSeq,
  current: () => openSeq,
});

/** The show catalogue and what plays an episode, on the same player. */
const mockShows = createMockShows({
  state: () => playerEmitter.last,
  patch: patchPlayer,
  open: () => ++openSeq,
  current: () => openSeq,
});

/** Continue watching: the two catalogues' places, merged. */
const mockContinue = createMockContinue(mockMovies, mockShows);

export const mockApi = {
  categories: async () => {
    await delay(120);
    return getCatalog().categories;
  },

  channels: async (categoryId: string | null, offset: number, limit: number) => {
    await delay(40);
    const c = getCatalog();
    if (categoryId === null) return c.rows.slice(offset, offset + limit);
    const s = c.span.get(categoryId);
    if (!s) throw "unknown category";
    const [start, len] = s;
    if (offset >= len) return [];
    return c.rows.slice(start + offset, start + Math.min(len, offset + limit));
  },

  channelsByIds: async (ids: number[]) => {
    await delay(20);
    if (ids.length === 0) return [];
    const want = new Set(ids);
    // Ids that no longer exist are simply absent, same as the backend: a
    // remembered channel can vanish between sessions.
    return getCatalog().rows.filter((r) => !r.isSeparator && want.has(r.streamId));
  },

  alternativesFor: async (streamId: number) => {
    await delay(20);
    const rows = getCatalog().rows;
    const self = rows.find((r) => r.streamId === streamId);
    const id = self ? mockGuideId(self) : null;
    if (!id) return [];
    return rankAlternatives(
      rows.filter((r) => mockGuideId(r) === id),
      streamId,
    );
  },

  channelNumbers: async (): Promise<[string, number][]> => {
    await delay(80);
    const out: [string, number][] = [];
    for (const r of getCatalog().rows) {
      if (r.isSeparator || r.num === null) continue;
      out.push([r.numLabel ?? String(r.num), r.streamId]);
    }
    return out;
  },

  search: async (query: string, limit: number) => {
    await delay(60);
    // Each alternative's words all match, as nucleo's do; any alternative
    // will do.
    const alts = alternatives(query.toLowerCase()).map((a) => a.split(/\s+/));
    if (alts.length === 0) return [];
    const out: ChannelDto[] = [];
    for (const r of getCatalog().rows) {
      // Separators are never search hits.
      if (r.isSeparator) continue;
      const name = r.name.toLowerCase();
      if (alts.some((words) => words.every((w) => subsequence(w, name)))) {
        out.push(r);
        if (out.length >= limit) break;
      }
    }
    return out;
  },

  searchProgrammes: async (query: string, offset: number, limit: number) => {
    await delay(70);
    const alts = alternatives(query)
      .map(foldWords)
      .filter((w) => w.length > 0);
    if (alts.length === 0) return [];
    const nowSec = Math.floor(Date.now() / 1000);
    // A handful of distinct titles, so each is judged once, not per airing.
    const verdicts = new Map<string, boolean>();
    const matches = (title: string) => {
      let v = verdicts.get(title);
      if (v === undefined) {
        const have = foldWords(title);
        v = alts.some((words) => words.every((w) => have.some((h) => h.startsWith(w))));
        verdicts.set(title, v);
      }
      return v;
    };
    const tier = { live: 0, upcoming: 1, catchup: 2 } as const;
    const hits: ProgrammeHitDto[] = [];
    for (const r of getCatalog().rows) {
      if (r.isSeparator || !r.nowPlaying) continue;
      // Finished airings only where an archive can still play them, as the
      // backend has it; two days ahead, as far as the guide scrolls.
      const back = r.hasArchive ? (r.archiveDays ?? 7) * 86400 : 0;
      for (const p of schedule(r, nowSec - back, nowSec + 2 * 86400)) {
        if (!matches(p.title)) continue;
        const airing = p.start > nowSec ? "upcoming" : p.stop > nowSec ? "live" : "catchup";
        hits.push({
          channel: airing === "live" ? { ...r, nowPlaying: p } : r,
          programme: p,
          airing,
        });
      }
    }
    hits.sort(
      (a, b) =>
        tier[a.airing] - tier[b.airing] ||
        (a.airing === "upcoming"
          ? a.programme.start - b.programme.start
          : b.programme.start - a.programme.start) ||
        (a.channel.num ?? 0) - (b.channel.num ?? 0),
    );
    return hits.slice(offset, offset + limit);
  },

  channelIndex: async (categoryId: string | null, streamId: number) => {
    await delay(20);
    const c = getCatalog();
    const [start, len] =
      categoryId === null ? [0, c.rows.length] : (c.span.get(categoryId) ?? [0, 0]);
    for (let i = 0; i < len; i++) {
      if (c.rows[start + i].streamId === streamId) return i;
    }
    return null;
  },

  guide: async (streamIds: number[], from: number, _to: number) => {
    await delay(50);
    const byId = new Map(getCatalog().rows.map((r) => [r.streamId, r]));
    const out: [number, ProgrammeDto[]][] = [];
    for (const id of streamIds) {
      const r = byId.get(id);
      if (!r?.nowPlaying) continue;
      const list: ProgrammeDto[] = [];
      for (let i = -6; i < 6; i++) {
        const dur = r.nowPlaying.stop - r.nowPlaying.start;
        const start = r.nowPlaying.start + i * dur;
        list.push({ start, stop: start + dur, title: r.nowPlaying.title, description: null });
      }
      out.push([id, list.filter((p) => p.stop > from)]);
    }
    return out;
  },

  guideRows: async (streamIds: number[], from: number, to: number) => {
    // The grid asks only for the rows it can see, so this is a short list even
    // though the catalogue is tens of thousands long. Counted, so a driver can
    // prove that: one request per scroll, tens of ids, never sixty thousand.
    const g = globalThis as Record<string, unknown>;
    const calls = (g.__guideRowsCalls as { n: number; biggest: number }) ?? {
      n: 0,
      biggest: 0,
    };
    calls.n += 1;
    calls.biggest = Math.max(calls.biggest, streamIds.length);
    g.__guideRowsCalls = calls;

    await delay(45);
    const byId = new Map(getCatalog().rows.map((r) => [r.streamId, r]));
    const out: GuideRowDto[] = [];
    for (const id of streamIds) {
      const r = byId.get(id);
      if (!r) continue;
      out.push({
        streamId: r.streamId,
        name: r.name,
        logoUrl: r.logoUrl,
        hasArchive: r.hasArchive,
        archiveDays: r.archiveDays,
        programmes: r.isSeparator ? [] : schedule(r, from, to),
      });
    }
    return out;
  },

  syncStatus: async () => syncEmitter.last,

  syncNow: async () => {
    // What `run_once` publishes when `load_account` finds nothing.
    if (mockAccount.source === "none") {
      syncEmitter.emit({ ...syncEmitter.last, phase: "failed", message: MOCK_NO_ACCOUNT });
      return;
    }
    syncEmitter.emit({ ...syncEmitter.last, phase: "categories", message: "fetching categories" });
    await delay(600);
    syncEmitter.emit({ ...syncEmitter.last, phase: "channels", message: "fetching channels" });
    await delay(900);
    // Landed as the phase ends, so the guide phase that follows reports the
    // new count the way the backend's next status would.
    catalogueLanded = true;
    syncEmitter.emit({
      ...syncEmitter.last,
      phase: "epg",
      message: "guide data",
      channelsTotal: CHANNEL_TOTAL,
    });
    await delay(700);
    syncEmitter.emit({
      phase: "idle",
      message: "",
      channelsTotal: CHANNEL_TOTAL,
      lastSynced: Math.floor(Date.now() / 1000),
    });
  },

  play: async (streamId: number) => {
    const row = getCatalog().rows.find((r) => r.streamId === streamId);
    const fail = row?.quality?.failedAt != null && !failedOnce.has(streamId);
    if (fail) failedOnce.add(streamId);
    void openStream(
      streamId,
      {
        width: row?.quality?.width ?? 1920,
        height: row?.quality?.height ?? 1080,
        fps: row?.quality?.fps ?? 50,
        codec: row?.quality?.codec ?? "h264",
      },
      { fail },
    );
  },

  playCatchup: async (streamId: number, _start: number, _durationMinutes: number) => {
    void openStream(streamId, { width: 1280, height: 720, fps: 50, codec: "h264" }, { live: false });
  },

  stop: async () => {
    openSeq++;
    patchPlayer({
      state: "stopped",
      streamId: null,
      error: null,
      ending: null,
      bufferedBack: 0,
      behindLive: 0,
      liveJump: null,
      joiningLive: false,
      reconnect: null,
      onDemand: null,
      finished: null,
    });
    patchOptions({ audio: [], subtitles: [] });
  },

  cancelReconnect: async () => {
    patchPlayer({ reconnect: null });
  },

  connectionStatus: async () => {
    await delay(60);
    return {
      max: 1,
      active: playerEmitter.last.state === "stopped" ? 0 : 1,
      ours: playerEmitter.last.state === "stopped" ? 0 : 1,
      counted: true,
      declared: false,
      expires: 1_893_456_000,
    };
  },

  togglePause: async () => {
    const p = playerEmitter.last;
    if (p.state === "playing") patchPlayer({ state: "paused" });
    else if (p.state === "paused") patchPlayer({ state: "playing" });
  },

  seekRelative: async (seconds: number) => {
    const p = playerEmitter.last;
    if (p.onDemand?.kind === "episode") return mockShows.seekRelative(seconds);
    if (p.onDemand) return mockMovies.seekRelative(seconds);
    const behindLive = Math.max(
      0,
      Math.min(p.bufferedBack, p.behindLive - seconds),
    );
    // Same hysteresis thresholds the backend uses, so the mock cannot make the
    // indicator look steadier than the real thing.
    patchPlayer({
      behindLive,
      atLiveEdge: p.atLiveEdge ? behindLive <= 8 : behindLive <= 3,
    });
  },

  // Reported as a jump when it went a second or more, as the backend does.
  seekLive: async () => {
    const by = playerEmitter.last.behindLive;
    patchPlayer({
      behindLive: 0,
      atLiveEdge: true,
      ...(by >= 1 ? { liveJump: { seconds: Math.round(by * 10) / 10, seq: ++mockJumps } } : {}),
    });
  },

  setVolume: async (volume: number) => patchPlayer({ volume }),
  setMuted: async (muted: boolean) => patchPlayer({ muted }),
  setCompressor: async (enabled: boolean) => patchPlayer({ compressor: enabled }),

  setVideoRect: async (rect: VideoRect) => {
    // Exposed so a browser driver can assert that docking actually moved the
    // video, which is otherwise invisible without mpv.
    (globalThis as Record<string, unknown>).__lastVideoRect = rect;
  },

  playerState: async () => playerEmitter.last,

  playerOptions: async () => optionsEmitter.last,
  setAudioTrack: async (id: number) =>
    patchOptions({ audio: optionsEmitter.last.audio.map((t) => ({ ...t, selected: t.id === id })) }),
  setSubtitleTrack: async (id: number | null) => {
    // The viewer's choice is the standing one from now, as the backend's.
    const chosen = id === null ? null : (optionsEmitter.last.subtitles.find((t) => t.id === id) ?? null);
    mockSubtitles = afterChoosing(chosen, mockSubtitles);
    // Off hides the track and keeps it, as the backend's player does.
    if (id === null) return patchOptions({ subtitlesVisible: false });
    (globalThis as Record<string, unknown>).__mockSubtitleSwitches =
      ((globalThis as Record<string, unknown>).__mockSubtitleSwitches as number | undefined ?? 0) +
      (optionsEmitter.last.subtitles.some((t) => t.selected && t.id === id) ? 0 : 1);
    return patchOptions({
      subtitles: optionsEmitter.last.subtitles.map((t) => ({ ...t, selected: t.id === id })),
      subtitlesVisible: true,
    });
  },
  setAspect: async (aspect: AspectMode) => patchOptions({ aspect }),
  setDeinterlace: async (deinterlace: DeinterlaceMode) => patchOptions({ deinterlace }),

  // Synthetic but not static: the graphs are the point of the panel, and a
  // flat line would make a broken one look fine. The buffer breathes, the
  // rate wanders, and a frame is dropped now and then.
  playerStats: async () => {
    const p = playerEmitter.last;
    const t = Date.now() / 1000;
    const breathing = 14 + Math.sin(t / 7) * 6;
    mockDropped += Math.random() < 0.12 ? 1 : 0;
    // Synthesised backwards from now rather than accumulated, which is the
    // mock's version of what the backend does: the real one has been
    // recording all along, so the panel must open onto a full graph here
    // too, or the mock would make a fixed bug look unfixed.
    const history = Array.from({ length: MOCK_HISTORY }, (_, i) => {
      const at = t - (MOCK_HISTORY - 1 - i) * 0.5;
      const buffer = 14 + Math.sin(at / 7) * 6;
      return {
        buffer: Math.max(0, buffer),
        speed: 1_100_000 + Math.sin(at / 2) * 400_000,
        // Bursty rather than uniform: a graph of evenly spaced drops looks
        // like a measurement error, and clustered ones are what a stutter
        // actually produces.
        dropped: i % 17 === 0 ? 2 : i % 5 === 0 ? 1 : 0,
        starved: buffer < 9,
      };
    });
    return {
      container: "mpegts",
      videoCodec: "h264 (High)",
      audioCodec: "aac",
      hwdec: "vaapi",
      pixelFormat: "yuv420p",
      primaries: "bt.709",
      gamma: "bt.1886",
      videoBitrate: 8_200_000 + Math.sin(t / 3) * 900_000,
      audioBitrate: 128_000,
      measuredFps: 49.8 + Math.sin(t) * 0.3,
      containerFps: 50,
      audioSamplerate: 48_000,
      audioChannels: 2,
      cacheDuration: Math.max(0, breathing),
      cacheSpeed: 1_100_000 + Math.sin(t / 2) * 400_000,
      demuxerBytes: Math.round(t * 1_000_000) % 400_000_000,
      buffering: breathing < 9,
      cacheBufferingPercent: 100,
      droppedFrames: mockDropped,
      decoderDroppedFrames: 0,
      avsync: Math.sin(t / 5) * 0.004,
      behindLive: p.behindLive,
      bufferedBack: p.bufferedBack,
      history,
      // Counted the way the backend does: each run of starved samples is one.
      stalls: history.filter((s, i) => s.starved && !(history[i - 1]?.starved ?? false))
        .length,
    };
  },

  // A synthetic frame, drawn rather than decoded, so the scrubber's preview
  // can be seen and driven without mpv. Same shape as the real answer: it
  // snaps to a four-second grid and refuses anything outside the buffer,
  // because those are the two things the interface has to cope with.
  thumbnailAt: async (behindLive: number) => {
    const p = playerEmitter.last;
    if (!Number.isFinite(behindLive) || behindLive < 0) return null;
    if (behindLive > p.bufferedBack) return null;
    const snapped = Math.round(behindLive / 4) * 4;
    const c = document.createElement("canvas");
    c.width = 160;
    c.height = 90;
    const g = c.getContext("2d");
    if (!g) return null;
    // Something that visibly changes with the timestamp, so a preview stuck
    // on one frame is obvious rather than plausible.
    const hue = (snapped * 11) % 360;
    g.fillStyle = `hsl(${hue} 55% 28%)`;
    g.fillRect(0, 0, 160, 90);
    g.fillStyle = `hsl(${(hue + 40) % 360} 60% 46%)`;
    g.fillRect(0, 60, 160, 30);
    g.fillStyle = "#fff";
    g.font = "bold 20px system-ui, sans-serif";
    g.textAlign = "center";
    g.fillText(`-${snapped}s`, 80, 40);
    return { dataUrl: c.toDataURL("image/png"), behindLive: snapped };
  },

  account: async () => {
    await delay(40);
    return { ...mockAccount, path: "~/.config/zapper/account.json" };
  },

  setAccount: async (host: string, username: string, password: string) => {
    await delay(60);
    if (host.trim() === "") throw "a host is required";
    if (username.trim() === "") throw "a username is required";
    if (password === "" && !mockAccount.hasPassword) throw "a password is required";
    mockAccount = {
      ...mockAccount,
      host: host.trim(),
      username: username.trim(),
      hasPassword: mockAccount.hasPassword || password !== "",
      source: "zapper",
      path: "~/.config/zapper/account.json",
      kind: "xtream",
    };
    // As the backend does: a new account is what a sync was waiting for.
    void mockApi.syncNow();
  },

  setM3uAccount: async (playlist: string | null, epgUrls: string[] | null, userAgent: string) => {
    await delay(60);
    if (playlist === null && mockAccount.playlist === "") throw "a playlist URL or file is required";
    // The mock's own redaction, so a saved secret-bearing URL comes back the
    // way the real backend would send it.
    const redact = (u: string) =>
      u.replace(/((?:username|password|token|key)=)[^&]*/gi, "$1<redacted>");
    const nextPlaylist = playlist ?? mockAccount.playlist;
    const nextEpg = epgUrls ?? mockAccount.epgUrls;
    mockAccount = {
      ...mockAccount,
      source: "zapper",
      kind: "m3u",
      playlist: redact(nextPlaylist),
      // Kept means whatever it was; typed means whether it needed redacting.
      playlistRedacted:
        playlist === null ? mockAccount.playlistRedacted : redact(nextPlaylist) !== nextPlaylist,
      epgUrls: nextEpg.map(redact),
      epgRedacted:
        epgUrls === null ? mockAccount.epgRedacted : nextEpg.some((u) => redact(u) !== u),
      userAgent: userAgent.trim(),
    };
    void mockApi.syncNow();
  },

  setHdhomerunAccount: async (device: string, epgUrls: string[] | null) => {
    await delay(60);
    if (device.trim() === "") throw "an HDHomeRun address is required";
    const nextEpg = epgUrls ?? mockAccount.hdhomerunEpgUrls;
    mockAccount = {
      ...mockAccount,
      source: "zapper",
      kind: "hdhomerun",
      hdhomerunDevice: device.trim(),
      hdhomerunEpgUrls: nextEpg.map(mockRedact),
      hdhomerunEpgRedacted:
        epgUrls === null ? mockAccount.hdhomerunEpgRedacted : nextEpg.some((u) => mockRedact(u) !== u),
    };
    void mockApi.syncNow();
  },

  setTvheadendAccount: async (url: string, username: string, password: string) => {
    await delay(60);
    if (url.trim() === "") throw "a Tvheadend address is required";
    if (/^[a-z]+:\/\/[^/]*@/i.test(url.trim()))
      throw "put the username and password in their own fields, not in the address";
    const user = username.trim();
    if (user !== "" && password === "" && !mockAccount.tvheadendHasPassword)
      throw "a password is required with a username (leave both empty if the server needs no login)";
    mockAccount = {
      ...mockAccount,
      source: "zapper",
      kind: "tvheadend",
      tvheadendUrl: url.trim(),
      tvheadendUsername: user,
      tvheadendHasPassword: user !== "" && (password !== "" || mockAccount.tvheadendHasPassword),
    };
    void mockApi.syncNow();
  },

  testAccount: async () => {
    await delay(300);
    if (mockAccount.kind === "hdhomerun") {
      return "HDHomeRun FLEX 4K (HDFX-4K): 62 channels, 4 tuners · 3 copy-protected, not listed";
    }
    if (mockAccount.kind === "tvheadend") {
      // Typing "nopersist" as the user shows the message a server without
      // persistent authentication gets.
      if (mockAccount.tvheadendUsername === "nopersist")
        throw "Tvheadend accepted this login but will not give out stream links that keep working. In Tvheadend, open Configuration → Users → Passwords, edit this user, set “Persistent authentication” to Enable, and save; then sync again.";
      const login = mockAccount.tvheadendUsername === "" ? " · no login" : "";
      return `Tvheadend 4.3-2183~g1b5b5ca: 212 channels${login}`;
    }
    if (mockAccount.kind === "m3u") {
      const guide =
        mockAccount.epgUrls.length === 0 ? "no guide configured" : `${mockAccount.epgUrls.length} guides OK`;
      return `Playlist OK: 412 channels in the first 600 lines of 24 MB · ${guide}`;
    }
    if (!mockAccount.hasPassword) throw "the server rejected these credentials (no password)";
    return "Active · 1 of 2 connections in use";
  },

  // The form as typed, checked against nothing but itself: the saved account
  // is not read and not changed, as with the real command. A password of
  // "wrong" is refused, so both answers can be seen; a blank one stands for
  // the one on file, which this kind may not have.
  testAccountCandidate: async (c: AccountCandidateDto) => {
    await delay(300);
    switch (c.kind) {
      case "hdhomerun":
        if (c.device.trim() === "") throw "an HDHomeRun address is required";
        return `HDHomeRun FLEX 4K (HDFX-4K) at ${c.device.trim()}: 62 channels, 4 tuners`;
      case "tvheadend": {
        if (c.username.trim() === "nopersist")
          throw "Tvheadend accepted this login but will not give out stream links that keep working. In Tvheadend, open Configuration → Users → Passwords, edit this user, set “Persistent authentication” to Enable, and save; then sync again.";
        if (c.password === "wrong") throw "Tvheadend refused this username and password.";
        const login = c.username.trim() === "" ? " · no login" : "";
        return `Tvheadend 4.3-2183~g1b5b5ca: 212 channels${login}`;
      }
      case "m3u": {
        const guides = c.epgUrls ?? mockAccount.epgUrls;
        const guide = guides.length === 0 ? "no guide configured" : `${guides.length} guides OK`;
        return `Playlist OK: 412 channels in the first 600 lines of 24 MB · ${guide}`;
      }
      case "xtream":
        if (c.password === "" && (!mockAccount.hasPassword || mockAccount.source === "none"))
          throw "a password is required";
        if (c.password === "wrong") throw "the server rejected these credentials (Disabled)";
        return `Active · 1 of 2 connections in use (${c.host.trim()} as ${c.username.trim()})`;
    }
  },

  settings: async () => {
    await delay(80);
    return structuredClone(storedSettings);
  },

  setSettings: async (next: SettingsDto) => {
    await delay(30);
    storedSettings = structuredClone(next);
    try {
      sessionStorage.setItem(
        MOCK_SUBTITLES_KEY,
        JSON.stringify({ subtitlesOn: next.subtitlesOn, subtitleLanguage: next.subtitleLanguage }),
      );
    } catch {
      // No storage: the choice lasts as long as the page.
    }
    // Exposed so a driver can prove a change actually reached the backend
    // rather than only moving the control.
    (globalThis as Record<string, unknown>).__mockSettings = storedSettings;
  },

  setSubtitleStyle: async (style: SubtitleStyleDto) => {
    await delay(20);
    storedSettings = { ...storedSettings, subtitle: structuredClone(style) };
    (globalThis as Record<string, unknown>).__mockSubtitleStyle = storedSettings.subtitle;
  },

  favourites: async (limit: number) => {
    await delay(40);
    return favouriteRows(limit);
  },

  recentlyWatched: async (limit: number) => {
    await delay(40);
    return recentRows(limit);
  },

  moviesSummary: mockMovies.moviesSummary,
  movieCategories: mockMovies.movieCategories,
  movies: mockMovies.movies,
  movieDetails: mockMovies.movieDetails,
  similarMovies: mockMovies.similarMovies,
  setMovieMarks: mockMovies.setMovieMarks,
  playMovie: mockMovies.playMovie,
  // Whichever kind of title is open moves.
  seekTo: (seconds: number) =>
    playerEmitter.last.onDemand?.kind === "episode"
      ? mockShows.seekTo(seconds)
      : mockMovies.seekTo(seconds),

  showsSummary: mockShows.showsSummary,
  seriesCategories: mockShows.seriesCategories,
  series: mockShows.series,
  seriesDetails: mockShows.seriesDetails,
  setSeriesMarks: mockShows.setSeriesMarks,
  setEpisodeMarks: mockShows.setEpisodeMarks,
  playingEpisode: mockShows.playingEpisode,
  playEpisode: mockShows.playEpisode,
  playNextEpisode: mockShows.playNextEpisode,

  continueWatching: mockContinue.continueWatching,
  removeFromContinue: mockContinue.removeFromContinue,
  movieMarks: mockContinue.movieMarks,
  episodeMarks: mockContinue.episodeMarks,

  forgetActivity: async (streamId: number) => {
    await delay(20);
    if (!activitySeeded) seedActivity(getCatalog().rows);
    activity.delete(streamId);
    // Exposed so a driver can prove the row went because the backend was told
    // to forget it, not because the list re-rendered.
    (globalThis as Record<string, unknown>).__forgotten = [
      ...(((globalThis as Record<string, unknown>).__forgotten as number[]) ?? []),
      streamId,
    ];
  },

  activityRows: async (): Promise<ActivityRowDto[]> => {
    await delay(30);
    const c = getCatalog();
    if (!activitySeeded) seedActivity(c.rows);
    const byId = new Map(c.rows.map((r) => [r.streamId, r]));
    const now = Math.floor(Date.now() / 1000);
    return [...activity]
      .map(([streamId, a]) => ({
        streamId,
        name: byId.get(streamId)?.cleanName ?? null,
        watchSeconds: a.watchSeconds,
        lastWatched: now - a.ago,
        openCount: a.openCount,
      }))
      .sort((x, y) => y.watchSeconds - x.watchSeconds || x.streamId - y.streamId);
  },

  importActivity: async (rows: ActivityRowDto[]) => {
    await delay(30);
    if (!activitySeeded) seedActivity(getCatalog().rows);
    const now = Math.floor(Date.now() / 1000);
    // The backend's merge: the larger of each figure, per channel.
    for (const r of rows) {
      const had = activity.get(r.streamId);
      const ago = r.lastWatched === null ? (had?.ago ?? 365 * 86_400) : Math.max(0, now - r.lastWatched);
      activity.set(r.streamId, {
        watchSeconds: Math.max(had?.watchSeconds ?? 0, r.watchSeconds),
        ago: had ? Math.min(had.ago, ago) : ago,
        openCount: Math.max(had?.openCount ?? 0, r.openCount),
      });
    }
    return rows.length;
  },

  /** A browser download in place of the save dialog. The account, when asked
   *  for, is added here as the backend adds it — a stand-in with a fake
   *  password, never anything real. */
  exportBackup: async (document: unknown, includeAccount: boolean, fileName: string) => {
    await delay(20);
    const doc = { ...(document as Record<string, unknown>) };
    delete doc.account;
    if (includeAccount) {
      doc.account = {
        kind: mockAccount.kind,
        host: mockAccount.host,
        username: mockAccount.username,
        password: "mock-password",
      };
    }
    const text = `${JSON.stringify(doc, null, 2)}\n`;
    (globalThis as Record<string, unknown>).__mockExported = text;
    const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
    const a = document_().createElement("a");
    a.href = url;
    a.download = fileName;
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    return fileName;
  },

  /** A file picker in place of the open dialog; the account is taken out and
   *  kept here, as the backend keeps it. */
  openBackup: async (): Promise<BackupFileDto | null> => {
    const file = await pickFile();
    if (!file) return null;
    const text = await file.text();
    let value: unknown;
    try {
      value = JSON.parse(text.replace(/^\uFEFF/, ""));
    } catch (e) {
      throw `this file is not JSON (${String(e)})`;
    }
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
      throw "this file is not a zapper backup";
    }
    const doc = { ...(value as Record<string, unknown>) };
    if (doc.format !== "zapper-backup") throw "this file is not a zapper backup";
    const account = doc.account as Record<string, unknown> | null | undefined;
    delete doc.account;
    mockStagedAccount = null;
    let summary: BackupFileDto["account"] = null;
    let accountError: string | null = null;
    if (account) {
      const kind = (account.kind as string | undefined) || "xtream";
      if (!["xtream", "m3u", "hdhomerun", "tvheadend"].includes(kind)) {
        accountError = `the account in this file is of a kind this version does not know (${kind})`;
      } else {
        mockStagedAccount = account;
        summary = {
          kind: kind as AccountDto["kind"],
          description: `${String(account.host ?? account.playlist ?? "")} as ${String(account.username ?? "")}`,
          hasSecret: typeof account.password === "string" && account.password !== "",
        };
      }
    }
    return { name: file.name, document: doc, account: summary, accountError };
  },

  importBackupAccount: async () => {
    await delay(20);
    const a = mockStagedAccount;
    if (!a) throw "no account to import: open the backup again";
    mockStagedAccount = null;
    mockAccount = {
      ...mockAccount,
      kind: ((a.kind as string | undefined) || "xtream") as AccountDto["kind"],
      host: String(a.host ?? ""),
      username: String(a.username ?? ""),
      hasPassword: typeof a.password === "string" && a.password !== "",
    };
    (globalThis as Record<string, unknown>).__mockImportedAccount = a;
  },

  applyPlaybackSettings: async (next: SettingsDto) => {
    await delay(20);
    (globalThis as Record<string, unknown>).__mockPlaybackApplied = {
      rewindMb: next.rewindMb,
      hardwareDecode: next.hardwareDecode,
      toneMapHdr: next.toneMapHdr,
      liveStart: next.liveStart,
    };
    mockLiveStart = next.liveStart;
    mockSubtitles = { subtitlesOn: next.subtitlesOn, subtitleLanguage: next.subtitleLanguage };
  },

  setMediaSession: async (session: MediaSessionDto | null) => {
    // Exposed so a driver can read what the desktop's controls would show.
    (globalThis as Record<string, unknown>).__mockMediaSession = session;
  },

  updateStatus: async (): Promise<UpdateStatusDto> => {
    await delay(20);
    return updateEmitter.last;
  },

  updateCheck: async (): Promise<UpdateStatusDto> => {
    const s = updateEmitter.last.state.state;
    if (s === "notConfigured") return updateEmitter.last;
    if (["downloading", "ready", "installing", "installed"].includes(s)) {
      return updateEmitter.last;
    }
    const before = updateEmitter.last.state;
    // As the backend does: an update already on offer stays on screen while
    // it is checked again; only a check with nothing to show says Checking.
    if (!("release" in before && before.release)) setUpdate({ state: { state: "checking" } });
    await delay(600);
    if (UPDATES_MODE === "offline") {
      setUpdate({ state: before });
      throw "GitHub answered HTTP 503";
    }
    setUpdate({
      lastChecked: Math.floor(Date.now() / 1000),
      state:
        UPDATES_MODE === "current"
          ? { state: "upToDate" }
          : { state: "available", release: MOCK_RELEASE },
    });
    return updateEmitter.last;
  },

  updateDownload: async (): Promise<UpdateStatusDto> => {
    const s = updateEmitter.last.state;
    if (s.state !== "available" && !(s.state === "failed" && s.release)) {
      throw "There's no update to download";
    }
    const asset = MOCK_RELEASE.asset;
    if (!asset) {
      throw "This copy of zapper has no package to download: get the update from its release page";
    }
    let received = 0;
    setUpdate({ state: { state: "downloading", release: MOCK_RELEASE, received, total: asset.size } });
    window.clearInterval(updateTimer);
    updateTimer = window.setInterval(() => {
      received = Math.min(asset.size, received + asset.size / 12);
      if (updateEmitter.last.state.state !== "downloading") {
        window.clearInterval(updateTimer);
        return;
      }
      if (received >= asset.size) {
        window.clearInterval(updateTimer);
        setUpdate({ state: { state: "ready", release: MOCK_RELEASE } });
      } else {
        setUpdate({
          state: { state: "downloading", release: MOCK_RELEASE, received, total: asset.size },
        });
      }
    }, 150);
    return updateEmitter.last;
  },

  updateCancel: async (): Promise<UpdateStatusDto> => {
    window.clearInterval(updateTimer);
    if (updateEmitter.last.state.state === "waitingForRecording") {
      setUpdate({ state: { state: "ready", release: MOCK_RELEASE } });
    } else if (updateEmitter.last.state.state === "downloading") {
      setUpdate({ state: { state: "available", release: MOCK_RELEASE } });
    }
    return updateEmitter.last;
  },

  updateInstall: async (when: InstallWhen = "ask"): Promise<InstallOutcomeDto> => {
    if (updateEmitter.last.state.state !== "ready") throw "There's no verified update to install";
    if (UPDATES_MODE === "recording") {
      const until = Math.floor(Date.now() / 1000) + 600;
      if (when === "ask") return { outcome: "recording", until, title: "The Late Film" };
      if (when === "whenIdle") {
        setUpdate({ state: { state: "waitingForRecording", release: MOCK_RELEASE, until } });
        // Long enough to see; Cancel puts it back to ready.
        await delay(4000);
        // Read again: Cancel may have changed it meanwhile.
        const now = (updateEmitter.last as UpdateStatusDto).state;
        if (now.state !== "waitingForRecording") return { outcome: "notNow" };
      }
      setUpdate({ state: { state: "installing", release: MOCK_RELEASE } });
      await delay(600);
      return { outcome: "quitting" };
    }
    setUpdate({ state: { state: "installing", release: MOCK_RELEASE } });
    await delay(900);
    if (UPDATES_MODE === "manual") {
      setUpdate({ state: { state: "ready", release: MOCK_RELEASE } });
      return {
        outcome: "manual",
        command: "sudo apt install /home/demo/.cache/zapper/updates/zapper_0.2.0_amd64.deb",
        reason:
          "zapper couldn't get permission to install it: the password prompt was cancelled, or there's no polkit agent to ask.",
        output: null,
      };
    }
    setUpdate({ state: { state: "installed", release: MOCK_RELEASE } });
    return { outcome: "installed" };
  },

  updateRestart: async () => {
    if (updateEmitter.last.state.state !== "installed") throw "No update has been installed";
    // A browser tab cannot restart itself into a new version; recording the
    // request is the honest stand-in.
    (globalThis as Record<string, unknown>).__mockRestarted = true;
  },

  updateOpenPage: async () => {
    if (updateEmitter.last.state.state === "notConfigured") {
      throw "Updates aren't configured for this build, so there's no release page to open.";
    }
    (globalThis as Record<string, unknown>).__mockOpenedPage = MOCK_RELEASE.url;
  },

  ...mockRecordingApi,
};

// ------------------------------------------------------------------ window

/** The undecorated window's own controls. The real ones are Tauri calls that
 *  reject outright in a plain browser, which would make the toolbar — the one
 *  piece of chrome that is always on screen — the one piece that cannot be
 *  driven without Tauri. So they are faked, and every call is recorded on
 *  `globalThis.__windowCalls` so a driver can prove the button did something
 *  rather than only that it was clickable.
 *
 *  `toggleMaximise` really does resize something: the mock marks `<html>` so
 *  the page can be seen in both states, and `isMaximised` answers from the
 *  same flag the button set. */
let maximised = false;

function recordWindowCall(name: string) {
  const g = globalThis as Record<string, unknown>;
  const log = (g.__windowCalls as string[]) ?? [];
  log.push(name);
  g.__windowCalls = log;
}

export const mockWindow = {
  startDragging: async () => {
    recordWindowCall("startDragging");
  },
  startResizeDragging: async (direction: string) => {
    recordWindowCall(`startResizeDragging:${direction}`);
  },
  minimise: async () => {
    recordWindowCall("minimise");
  },
  toggleMaximise: async () => {
    recordWindowCall("toggleMaximise");
    maximised = !maximised;
    document.documentElement.dataset.mockMaximised = maximised ? "1" : "0";
    // A real maximise resizes the window, and that resize is how the rest of
    // the app finds out. Faking the state without the event would leave the
    // button's own icon stale, which is the bug this state exists to avoid.
    window.dispatchEvent(new Event("resize"));
  },
  isMaximised: async () => maximised,
  close: async () => {
    recordWindowCall("close");
  },
  titleBarDoubleClick: async () => {
    recordWindowCall("titleBarDoubleClick");
  },
};

// Assertions a browser driver can run against the live page: no light surface
// anywhere, and nothing painted over the video rect. Mock-only, like
// `__lastVideoRect` — the real app never reads these.
if (typeof globalThis !== "undefined") {
  const g = globalThis as Record<string, unknown>;
  g.__audit = {
    lightSurfaces: (minSize?: number) => auditLightSurfaces(minSize),
    overVideo: () => auditOverVideo(g.__lastVideoRect as VideoRect | undefined),
  };
}

/** A broadcast-looking still for the stand-in video plane, so screenshots of
 *  the mock show a picture rather than a test pattern. It is fetched by URL
 *  from the dev server (which serves the project root), never imported, so it
 *  stays out of the production bundle; the caller only reaches this under
 *  `import.meta.env.DEV`. The pattern from `.mock-video` stays until the image
 *  has loaded, and stays for good if it never does. Credit: docs/licensing.md. */
export function paintMockStill(el: HTMLElement): void {
  const url = "/docs/images/broadcast-still.jpg";
  const img = new Image();
  img.onload = () => {
    // Contained on black, the way mpv letterboxes a picture whose shape
    // doesn't match the rect.
    el.style.background = `#000 url("${url}") center / contain no-repeat`;
  };
  img.src = url;
}

/** The account from the last backup opened, as the backend stages it. */
let mockStagedAccount: Record<string, unknown> | null = null;

// `document` is shadowed by the export's parameter above.
const document_ = () => globalThis.document;

/** The browser's file picker, resolved with nothing when it is dismissed. */
function pickFile(): Promise<File | null> {
  return new Promise((resolve) => {
    const input = document_().createElement("input");
    input.type = "file";
    input.accept = ".json,application/json";
    input.addEventListener("change", () => resolve(input.files?.[0] ?? null));
    input.addEventListener("cancel", () => resolve(null));
    input.click();
  });
}

/** `?backend=broken` makes every command reject the way the real backend does
 *  today, so the degraded UI can be seen rather than imagined. */
const BROKEN =
  typeof location !== "undefined" && /(^|[?&])backend=broken(&|$)/.test(location.search);

export const mockBackend: typeof mockApi = BROKEN
  ? (Object.fromEntries(
      Object.keys(mockApi).map((k) => [k, () => Promise.reject("not implemented")]),
    ) as unknown as typeof mockApi)
  : mockApi;

export const onMockSyncStatus = (f: (s: SyncStatusDto) => void) =>
  Promise.resolve(syncEmitter.on(f));

export const onMockPlayerState = (f: (s: PlayerStateDto) => void) =>
  Promise.resolve(playerEmitter.on(f));

export const onMockPlayerOptions = (f: (s: PlayerOptionsDto) => void) =>
  Promise.resolve(optionsEmitter.on(f));

export const onMockUpdate = (f: (s: UpdateStatusDto) => void) =>
  Promise.resolve(updateEmitter.on(f));

export const onMockChannelQuality = (f: (q: ChannelQualityDto) => void) =>
  Promise.resolve(qualityEmitter.on(f));

/** The desktop's media keys, which a browser has no way to press: a driver
 *  presses one with `__mockMediaKey("next")`. Pressed only when the stored
 *  setting is on, as the real registration only exists then. */
const mediaKeyEmitter = new Emitter<MediaKey>("toggle");
(globalThis as Record<string, unknown>).__mockMediaKey = (k: MediaKey) => {
  if (storedSettings.mediaControls) mediaKeyEmitter.emit(k);
  return storedSettings.mediaControls;
};

export const onMockMediaKey = (f: (k: MediaKey) => void) =>
  Promise.resolve(mediaKeyEmitter.on(f));

/** Words with case and accents folded away, split and folded the way the
 *  backend's FTS5 tokenizer treats titles. */
function foldWords(text: string): string[] {
  return text
    .normalize("NFD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .split(/[^\p{L}\p{N}]+/u)
    .filter((w) => w !== "");
}

/** Cheap subsequence match, standing in for nucleo. Ranking is not modelled;
 *  the real matcher lives in Rust and the UI must not reimplement it. */
function subsequence(needle: string, haystack: string): boolean {
  let i = 0;
  for (let j = 0; j < haystack.length && i < needle.length; j++) {
    if (haystack[j] === needle[i]) i++;
  }
  return i === needle.length;
}
