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
  CategoryDto,
  ChannelDto,
  GuideRowDto,
  PlayerStateDto,
  ProgrammeDto,
  QualityDto,
  SettingsDto,
  SubtitleStyleDto,
  SyncStatusDto,
  VideoRect,
} from "./types";

const CHANNEL_TOTAL = 60000;
const CATEGORY_TOTAL = 768;
/** Roughly one row in sixty is a panel-injected divider. */
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
  "EVENTS",
  "ALL DAY SHOWS",
  "RADIO",
  "WEATHER",
  "SHOPPING",
  "CLASSIC TV",
] as const;

const NETWORKS = [
  "HARBOUR SPORTS ONE",
  "HSN",
  "CRC",
  "CVT",
  "GLOBAL",
  "NWN ONE",
  "NWN TWO",
  "BRT",
  "CHANNEL 4",
  "SUMMIT SPORTS MAIN",
  "XSPN",
  "VIXEN SPORTS",
  "WNN",
  "WSNBC",
  "LEDGER",
  "EXPLORER",
  "CHRONICLE",
  "ATLAS EXPLORER",
  "AMV",
  "TNX",
  "CBO",
  "SHOWCASE",
  "STARLIT",
  "REELHOUSE",
  "FABLE",
  "DOODLE NETWORK",
  "NICKELPLAY",
  "MXV",
  "VJ1",
  "CHUCKLE CENTRAL",
  "KITCHEN NETWORK",
  "HMTV",
  "TLV",
  "SCIFY",
  "UNION NETWORK",
  "PARAGON",
  "ARTO",
  "NOVA+",
  "RVL",
  "PRO8",
  "LA NOVENA",
  "RAX 1",
  "TVX",
  "NPX 1",
  "SVS 6",
  "GLOBA",
  "TELEMAR",
  "NOVA PLUS",
  "CONTINENTAL SPORT",
  "DZNA",
] as const;

const SUFFIXES = ["", "", "", "", " HD", " FHD", " 4K", " RAW", " EAST", " WEST"];

const PROGRAMMES = [
  "Evening News",
  "Top Flight: Live",
  "The Night Desk",
  "Documentary: Deep Ocean",
  "Movie: The Long Road",
  "Local Weather",
  "Morning Show",
  "Classic Match Replay",
  "Wildlife Rescue",
  "Home Rescue",
  "Top 40 Countdown",
  "Grand Prix: Practice 2",
  "NIHL: Third Period",
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
  // A "4K" in the name means nothing; this is the case worth checking, so
  // the mock reproduces it verbatim for the badge to be checked against.
  if (name === "CA: HARBOUR SPORTS ONE 4K") {
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
        ? "A description the panel supplied, of roughly the length these actually run to in the wild."
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
        nowPlaying: rand() < 0.15 ? makeProgramme(nowSec) : null,
      });
    }

    span.set(id, [start, rows.length - start]);
    categories.push({ id, name, channelCount: count });
  }

  // One guaranteed specimen of the lying-name case, near the top of the list.
  const liar = rows.find((r) => !r.isSeparator);
  if (liar) {
    liar.name = "CA: HARBOUR SPORTS ONE 4K";
    liar.cleanName = "HARBOUR SPORTS ONE 4K";
    liar.quality = makeQuality("CA: HARBOUR SPORTS ONE 4K");
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
  volume: 100,
  muted: false,
  compressor: false,
  error: null,
});

const syncEmitter = new Emitter<SyncStatusDto>({
  phase: "idle",
  message: "",
  channelsTotal: CHANNEL_TOTAL,
  lastSynced: Math.floor(Date.now() / 1000) - 3600,
});

let bufferTimer: number | undefined;

/** Settings survive for the life of the page, so a save can be read back and
 *  a reload of the pane shows what was stored rather than the defaults. Not
 *  the contract's defaults on purpose: if the UI ever ignored what the backend
 *  returned and painted its own defaults, these values would show it. */
let storedSettings: SettingsDto = {
  rewindMinutes: 8,
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
};

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

async function openStream(streamId: number, quality: Partial<PlayerStateDto>) {
  patchPlayer({ state: "opening", streamId, error: null, bufferedBack: 0, behindLive: 0 });
  await delay(700);
  if (playerEmitter.last.streamId !== streamId) return;
  patchPlayer({ state: "playing", ...quality });
  startBufferClock();
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
    patchPlayer({ state: "stopped", streamId: null, bufferedBack: 0, behindLive: 0 });
  },

  togglePause: async () => {
    const p = playerEmitter.last;
    if (p.state === "playing") patchPlayer({ state: "paused" });
    else if (p.state === "paused") patchPlayer({ state: "playing" });
  },

  seekRelative: async (seconds: number) => {
    const p = playerEmitter.last;
    patchPlayer({
      behindLive: Math.max(0, Math.min(p.bufferedBack, p.behindLive - seconds)),
    });
  },

  seekLive: async () => patchPlayer({ behindLive: 0 }),

  setVolume: async (volume: number) => patchPlayer({ volume }),
  setMuted: async (muted: boolean) => patchPlayer({ muted }),
  setCompressor: async (enabled: boolean) => patchPlayer({ compressor: enabled }),

  setVideoRect: async (rect: VideoRect) => {
    // Exposed so a browser driver can assert that docking actually moved the
    // video, which is otherwise invisible without mpv.
    (globalThis as Record<string, unknown>).__lastVideoRect = rect;
  },

  playerState: async () => playerEmitter.last,

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

  applyPlaybackSettings: async (next: SettingsDto) => {
    await delay(20);
    (globalThis as Record<string, unknown>).__mockPlaybackApplied = {
      rewindMinutes: next.rewindMinutes,
      hardwareDecode: next.hardwareDecode,
      toneMapHdr: next.toneMapHdr,
    };
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

/** Cheap subsequence match, standing in for nucleo. Ranking is not modelled;
 *  the real matcher lives in Rust and the UI must not reimplement it. */
function subsequence(needle: string, haystack: string): boolean {
  let i = 0;
  for (let j = 0; j < haystack.length && i < needle.length; j++) {
    if (haystack[j] === needle[i]) i++;
  }
  return i === needle.length;
}
