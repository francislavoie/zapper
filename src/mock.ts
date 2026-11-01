// A fake backend, used only when Tauri is absent (i.e. `vite dev` in a plain
// browser). It exists so the UI can be driven and screenshotted without mpv or
// the Rust side, and so layout is exercised at a realistic scale — tens of
// thousands of channels across hundreds of categories is the whole reason the
// list is virtualized, and a 20-row fixture would hide every bug that matters.
//
// Shapes here mirror src/types.ts exactly. Nothing in this file is imported by
// the Tauri path at runtime beyond the module load itself.

import { auditLightSurfaces, auditOverVideo } from "./audit";
import type {
  AccountDto,
  CategoryDto,
  ChannelDto,
  FavouriteDto,
  GuideRowDto,
  PlayerStateDto,
  ProgrammeDto,
  QualityDto,
  SettingsDto,
  SubtitleStyleDto,
  SyncStatusDto,
  InstallKind,
  InstallOutcomeDto,
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
] as const;

// -------------------------------------------------------------- data building

export interface MockCatalog {
  categories: CategoryDto[];
  /** Every row, separators included, grouped by category in rail order. */
  rows: ChannelDto[];
  /** categoryId -> [start, length) into `rows`. */
  span: Map<string, [number, number]>;
}

function makeQuality(name: string): QualityDto | null {
  // A "4K" in the name means nothing, so the mock carries one channel that
  // claims it and delivers 720p, for the badge to be checked against.
  if (name === "CA: HARBOUR SPORTS 1 4K") {
    return {
      codec: "h264",
      width: 1280,
      height: 720,
      fps: 59.94,
      isHdr: false,
      okCount: 12,
      failCount: 1,
    };
  }
  // Most channels have never been opened. `null` is unknown, not bad.
  if (rand() > 0.09) return null;
  const r = rand();
  const [w, h] =
    r < 0.5 ? [1920, 1080] : r < 0.85 ? [1280, 720] : r < 0.95 ? [3840, 2160] : [720, 576];
  return {
    codec: rand() < 0.75 ? "h264" : "hevc",
    width: w,
    height: h,
    fps: rand() < 0.6 ? 50 : 59.94,
    isHdr: h >= 2160 && rand() < 0.4,
    okCount: Math.floor(rand() * 40),
    failCount: rand() < 0.15 ? Math.floor(rand() * 20) : 0,
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
        quality: makeQuality(full),
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
    liar.quality = makeQuality("CA: HARBOUR SPORTS 1 4K");
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
const getCatalog = (): MockCatalog => (catalog ??= buildCatalog());

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
});

const syncEmitter = new Emitter<SyncStatusDto>({
  phase: "idle",
  message: "",
  channelsTotal: CHANNEL_TOTAL,
  lastSynced: Math.floor(Date.now() / 1000) - 3600,
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

/** The mock's redaction, so a saved secret-bearing URL comes back the way
 *  the real backend would send it. */
const mockRedact = (u: string) => u.replace(/((?:username|password|token|key)=)[^&]*/gi, "$1<redacted>");
let bufferTimer: number | undefined;

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
};

// ----------------------------------------------------------------- updates

/** `?updates=` picks what the fake GitHub says, so every state of the
 *  update dialog can be looked at without a release:
 *
 *    available     a .deb install with 0.2.0 waiting (the default)
 *    source        a build from source: only the release page is offered
 *    manual        the install "fails" and hands back the command to run
 *    current       nothing newer
 *    unconfigured  the placeholder repository: no check ever happens
 *    offline       every check fails */
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
      : { name: "zapper_0.2.0_amd64.deb", size: 9_400_000 },
};

const updateEmitter = new Emitter<UpdateStatusDto>({
  current: "0.1.0",
  kind: (UPDATES_MODE === "source" ? "none" : "deb") as InstallKind,
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

/** Counts the mock's automatic catch-ups, like the backend's `seq`. */
let mockJumps = 0;

/** Bumped on every open, so a catch-up timer from the last stream cannot
 *  land on the next one. */
let openSeq = 0;

async function openStream(streamId: number, quality: Partial<PlayerStateDto>) {
  const mine = ++openSeq;
  patchPlayer({
    state: "opening",
    streamId,
    error: null,
    bufferedBack: 0,
    behindLive: 0,
    liveJump: null,
  });
  await delay(700);
  if (openSeq !== mine) return;
  // The connect burst: playback starts well behind live, as the real thing
  // does, and the backend's Settle brings it up once the burst is over —
  // reported as a jump, which is what the picture's corner announces.
  const behind = 14 + Math.round(Math.random() * 100) / 10;
  patchPlayer({ state: "playing", ...quality, behindLive: behind, bufferedBack: behind, atLiveEdge: false });
  startBufferClock();
  await delay(2500);
  if (openSeq !== mine || playerEmitter.last.state !== "playing") return;
  const cushion = 4;
  patchPlayer({
    behindLive: cushion,
    atLiveEdge: true,
    liveJump: { seconds: Math.round((behind - cushion) * 10) / 10, seq: ++mockJumps },
  });
}

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
    const q = query.trim().toLowerCase();
    if (q === "") return [];
    const out: ChannelDto[] = [];
    for (const r of getCatalog().rows) {
      // Separators are never search hits.
      if (r.isSeparator) continue;
      if (subsequence(q, r.name.toLowerCase())) {
        out.push(r);
        if (out.length >= limit) break;
      }
    }
    return out;
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
    syncEmitter.emit({ ...syncEmitter.last, phase: "categories", message: "fetching categories" });
    await delay(600);
    syncEmitter.emit({ ...syncEmitter.last, phase: "channels", message: "60,000 channels" });
    await delay(900);
    syncEmitter.emit({ ...syncEmitter.last, phase: "epg", message: "guide data" });
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
    void openStream(streamId, {
      width: row?.quality?.width ?? 1920,
      height: row?.quality?.height ?? 1080,
      fps: row?.quality?.fps ?? 50,
      codec: row?.quality?.codec ?? "h264",
    });
  },

  playCatchup: async (streamId: number, _start: number, _durationMinutes: number) => {
    void openStream(streamId, { width: 1280, height: 720, fps: 50, codec: "h264" });
  },

  stop: async () => {
    openSeq++;
    patchPlayer({
      state: "stopped",
      streamId: null,
      bufferedBack: 0,
      behindLive: 0,
      liveJump: null,
    });
  },

  togglePause: async () => {
    const p = playerEmitter.last;
    if (p.state === "playing") patchPlayer({ state: "paused" });
    else if (p.state === "paused") patchPlayer({ state: "playing" });
  },

  seekRelative: async (seconds: number) => {
    const p = playerEmitter.last;
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

  seekLive: async () => patchPlayer({ behindLive: 0, atLiveEdge: true }),

  setVolume: async (volume: number) => patchPlayer({ volume }),
  setMuted: async (muted: boolean) => patchPlayer({ muted }),
  setCompressor: async (enabled: boolean) => patchPlayer({ compressor: enabled }),

  setVideoRect: async (rect: VideoRect) => {
    // Exposed so a browser driver can assert that docking actually moved the
    // video, which is otherwise invisible without mpv.
    (globalThis as Record<string, unknown>).__lastVideoRect = rect;
  },

  playerState: async () => playerEmitter.last,

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

  settings: async () => {
    await delay(80);
    return structuredClone(storedSettings);
  },

  setSettings: async (next: SettingsDto) => {
    await delay(30);
    storedSettings = structuredClone(next);
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

  applyPlaybackSettings: async (next: SettingsDto) => {
    await delay(20);
    (globalThis as Record<string, unknown>).__mockPlaybackApplied = {
      rewindMb: next.rewindMb,
      hardwareDecode: next.hardwareDecode,
      toneMapHdr: next.toneMapHdr,
    };
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
    if (updateEmitter.last.state.state === "downloading") {
      setUpdate({ state: { state: "available", release: MOCK_RELEASE } });
    }
    return updateEmitter.last;
  },

  updateInstall: async (): Promise<InstallOutcomeDto> => {
    if (updateEmitter.last.state.state !== "ready") throw "There's no verified update to install";
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

export const onMockUpdate = (f: (s: UpdateStatusDto) => void) =>
  Promise.resolve(updateEmitter.on(f));

/** Cheap subsequence match, standing in for nucleo. Ranking is not modelled;
 *  the real matcher lives in Rust and the UI must not reimplement it. */
function subsequence(needle: string, haystack: string): boolean {
  let i = 0;
  for (let j = 0; j < haystack.length && i < needle.length; j++) {
    if (haystack[j] === needle[i]) i++;
  }
  return i === needle.length;
}
