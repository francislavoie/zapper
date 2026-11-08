// Settings › Data's backups: the file format, checking a file someone hands
// us, and saying what importing it would change. Pure; checked in
// src/__checks__/backup.test.mjs. The wiring is src/components/BackupPane.tsx
// and the file dialogs are the backend's (src-tauri/src/ipc/backup_cmds.rs).
//
// A backup is one JSON object:
//
//   format       "zapper-backup", so a stray JSON file is not mistaken for one
//   version      the format's version; a file from a newer zapper is refused
//                rather than half-read
//   exportedAt   when, ISO 8601, for the person looking at the file
//   settings     SettingsDto
//   categories   the rail's pins, hidden categories and sort
//   favourites   the watch history favourites are ranked from, per channel
//   account      only when asked for, and added and removed by the backend
//                alone, so its password never passes through here
//
// Every section is optional on import. A field of the wrong type is skipped
// and said so rather than failing the whole file; a field this build does
// not know (a newer build's) is ignored, the way the backend reads settings.

import type {
  ActivityRowDto,
  SettingsDto,
  SubtitleBackdrop,
  SubtitleStyleDto,
} from "../types";
import {
  CATEGORY_SORTS,
  SORT_LABEL,
  type CategorySort,
} from "./categoryOrder.ts";
import { LIVE_START_CHOICES, TIME_FORMAT_LABEL, clampSettings } from "./settings.ts";
import { languageName } from "./tracks.ts";

export const BACKUP_FORMAT = "zapper-backup";
export const BACKUP_VERSION = 1;

export interface CategoryPrefs {
  pins: string[];
  hidden: string[];
  sort: CategorySort;
}

export interface Backup {
  format: typeof BACKUP_FORMAT;
  version: number;
  exportedAt: string;
  settings: SettingsDto;
  categories: CategoryPrefs;
  favourites: ActivityRowDto[];
}

export function buildBackup(
  parts: { settings: SettingsDto; categories: CategoryPrefs; favourites: ActivityRowDto[] },
  now: Date,
): Backup {
  return {
    format: BACKUP_FORMAT,
    version: BACKUP_VERSION,
    exportedAt: now.toISOString(),
    settings: parts.settings,
    categories: {
      pins: [...parts.categories.pins],
      hidden: [...parts.categories.hidden],
      sort: parts.categories.sort,
    },
    favourites: parts.favourites,
  };
}

/** `zapper-settings-2026-11-01.json`, in local time: the day it was made, as
 *  the person making it would say it. */
export function backupFileName(now: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `zapper-settings-${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())}.json`;
}

// -------------------------------------------------------------- reading one

/** What a file holds, as far as it could be read. A section is absent when
 *  the file has none; a settings section holds only the fields that passed. */
export interface ParsedBackup {
  version: number;
  exportedAt: string | null;
  settings: Partial<Omit<SettingsDto, "subtitle">> & { subtitle?: Partial<SubtitleStyleDto> } | null;
  categories: Partial<CategoryPrefs> | null;
  favourites: ActivityRowDto[] | null;
}

export type ParseResult =
  | { ok: true; backup: ParsedBackup; warnings: string[] }
  | { ok: false; error: string };

/** `"language"` is a string, or null for none. */
type Kind = "number" | "boolean" | "string" | "language";

/** Every setting and its type. A `Record` over the DTO's keys, so a setting
 *  added to the contract and not here is a compile error, not a field that
 *  silently never travels. */
const SETTING_KINDS: Record<Exclude<keyof SettingsDto, "subtitle">, Kind> = {
  rewindMb: "number",
  hardwareDecode: "boolean",
  toneMapHdr: "boolean",
  guideHours: "number",
  syncIntervalHours: "number",
  language: "string",
  checkForUpdates: "boolean",
  mediaControls: "boolean",
  timeFormat: "string",
  liveStart: "string",
  simultaneousStreams: "number",
  subtitlesOn: "boolean",
  subtitleLanguage: "language",
};

const SUBTITLE_KINDS: Record<keyof SubtitleStyleDto, Kind> = {
  font: "string",
  size: "number",
  color: "string",
  backdrop: "string",
  backdropOpacity: "number",
  outlineSize: "number",
  shadowOffset: "number",
};

const BACKDROPS: readonly SubtitleBackdrop[] = ["background", "window", "outline", "shadow", "none"];

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

const fits = (v: unknown, kind: Kind) =>
  kind === "number"
    ? typeof v === "number" && Number.isFinite(v)
    : kind === "language"
      ? v === null || typeof v === "string"
      : typeof v === kind;

const KIND_WORDS: Record<Kind, string> = {
  number: "a number",
  boolean: "a boolean",
  string: "a string",
  language: "a string or null",
};

function readFields<K extends string>(
  raw: Record<string, unknown>,
  kinds: Record<K, Kind>,
  prefix: string,
  warnings: string[],
): Partial<Record<K, unknown>> {
  const out: Partial<Record<K, unknown>> = {};
  for (const key of Object.keys(kinds) as K[]) {
    if (!(key in raw)) continue;
    if (fits(raw[key], kinds[key])) out[key] = raw[key];
    else warnings.push(`Skipped ${prefix}${key}: not ${KIND_WORDS[kinds[key]]}.`);
  }
  return out;
}

function stringList(v: unknown, what: string, warnings: string[]): string[] | undefined {
  if (v === undefined) return undefined;
  if (!Array.isArray(v)) {
    warnings.push(`Skipped ${what}: not a list.`);
    return undefined;
  }
  const seen = new Set<string>();
  for (const x of v) if (typeof x === "string" && x !== "") seen.add(x);
  if (seen.size !== v.length) warnings.push(`Dropped unreadable or repeated entries from ${what}.`);
  return [...seen];
}

const wholeAtLeast = (v: unknown, min: number) =>
  typeof v === "number" && Number.isSafeInteger(v) && v >= min;

function readFavourites(v: unknown, warnings: string[]): ActivityRowDto[] | null {
  if (v === undefined) return null;
  if (!Array.isArray(v)) {
    warnings.push("Skipped favourites: not a list.");
    return null;
  }
  const rows = new Map<number, ActivityRowDto>();
  let bad = 0;
  for (const r of v) {
    if (
      !isObject(r) ||
      !wholeAtLeast(r.streamId, Number.MIN_SAFE_INTEGER) ||
      !wholeAtLeast(r.watchSeconds, 0) ||
      !(r.lastWatched === undefined || r.lastWatched === null || wholeAtLeast(r.lastWatched, 0)) ||
      !(r.openCount === undefined || wholeAtLeast(r.openCount, 0))
    ) {
      bad++;
      continue;
    }
    const row: ActivityRowDto = {
      streamId: r.streamId as number,
      name: typeof r.name === "string" ? r.name : null,
      watchSeconds: r.watchSeconds as number,
      lastWatched: (r.lastWatched as number | null | undefined) ?? null,
      openCount: (r.openCount as number | undefined) ?? 0,
    };
    // A channel listed twice is merged the way an import merges, rather
    // than one copy silently winning.
    const had = rows.get(row.streamId);
    rows.set(row.streamId, had ? mergeActivity(had, row) : row);
  }
  if (bad > 0) {
    warnings.push(`Skipped ${bad} unreadable watch-history ${bad === 1 ? "entry" : "entries"}.`);
  }
  return [...rows.values()];
}

/** Checks a file's contents. `doc` is whatever JSON the file held, without
 *  its account, which the backend has already taken out. */
export function parseBackup(doc: unknown): ParseResult {
  if (!isObject(doc) || doc.format !== BACKUP_FORMAT) {
    return { ok: false, error: "This file is not a zapper backup." };
  }
  const version = doc.version;
  if (!wholeAtLeast(version, 1)) {
    return { ok: false, error: "This backup does not say which version it is." };
  }
  if ((version as number) > BACKUP_VERSION) {
    return {
      ok: false,
      error: `This backup was made by a newer zapper (format ${version}). Update zapper to import it.`,
    };
  }

  const warnings: string[] = [];
  let settings: ParsedBackup["settings"] = null;
  if (doc.settings !== undefined) {
    if (!isObject(doc.settings)) {
      warnings.push("Skipped settings: not an object.");
    } else {
      const top = readFields(doc.settings, SETTING_KINDS, "", warnings);
      settings = top as NonNullable<ParsedBackup["settings"]>;
      const sub = doc.settings.subtitle;
      if (sub !== undefined) {
        if (!isObject(sub)) {
          warnings.push("Skipped subtitle: not an object.");
        } else {
          const s = readFields(sub, SUBTITLE_KINDS, "subtitle.", warnings);
          if ("backdrop" in s && !BACKDROPS.includes(s.backdrop as SubtitleBackdrop)) {
            warnings.push(`Skipped subtitle.backdrop: "${String(s.backdrop)}" is not one zapper draws.`);
            delete s.backdrop;
          }
          settings.subtitle = s as Partial<SubtitleStyleDto>;
        }
      }
    }
  }

  let categories: ParsedBackup["categories"] = null;
  if (doc.categories !== undefined) {
    if (!isObject(doc.categories)) {
      warnings.push("Skipped categories: not an object.");
    } else {
      const c = doc.categories;
      categories = {};
      const pins = stringList(c.pins, "pinned categories", warnings);
      const hidden = stringList(c.hidden, "hidden categories", warnings);
      if (pins) categories.pins = pins;
      if (hidden) categories.hidden = hidden;
      if (c.sort !== undefined) {
        if ((CATEGORY_SORTS as readonly unknown[]).includes(c.sort)) {
          categories.sort = c.sort as CategorySort;
        } else {
          warnings.push(`Skipped the category sort: "${String(c.sort)}" is not one zapper knows.`);
        }
      }
    }
  }

  return {
    ok: true,
    backup: {
      version: version as number,
      exportedAt: typeof doc.exportedAt === "string" ? doc.exportedAt : null,
      settings,
      categories,
      favourites: readFavourites(doc.favourites, warnings),
    },
    warnings,
  };
}

// ---------------------------------------------------- what it would change

/** Per channel, the larger of each figure: importing the same file twice
 *  changes nothing the second time, and a machine's own history is never
 *  lowered. The same rule the backend applies (`store::merge_activity`). */
export function mergeActivity(local: ActivityRowDto, incoming: ActivityRowDto): ActivityRowDto {
  const last =
    local.lastWatched === null
      ? incoming.lastWatched
      : incoming.lastWatched === null
        ? local.lastWatched
        : Math.max(local.lastWatched, incoming.lastWatched);
  return {
    streamId: local.streamId,
    name: local.name ?? incoming.name,
    watchSeconds: Math.max(local.watchSeconds, incoming.watchSeconds),
    lastWatched: last,
    openCount: Math.max(local.openCount, incoming.openCount),
  };
}

export interface Change {
  label: string;
  from: string;
  to: string;
}

export interface ImportPlan {
  settings: { next: SettingsDto; changes: Change[] } | null;
  categories: { next: CategoryPrefs; changes: Change[] } | null;
  favourites: {
    rows: ActivityRowDto[];
    /** Channels with no history here yet. */
    added: number;
    /** Channels whose history the file raises. */
    raised: number;
  } | null;
}

const onOff = (b: unknown) => (b ? "on" : "off");

/** How each setting is named and shown, in the words Settings uses. */
const SETTING_LABELS: Record<Exclude<keyof SettingsDto, "subtitle">, [string, (v: never) => string]> = {
  rewindMb: ["Rewind buffer", (v: number) => `${v.toLocaleString("en")} MB`],
  hardwareDecode: ["Hardware decode", onOff],
  toneMapHdr: ["Tone-map HDR to SDR", onOff],
  guideHours: ["Guide hours per screen", (v: number) => String(v)],
  syncIntervalHours: ["Catalogue refresh", (v: number) => `every ${v}h`],
  language: ["Language", (v: string) => v],
  checkForUpdates: ["Check for updates automatically", onOff],
  mediaControls: ["Media keys and desktop controls", onOff],
  timeFormat: ["Clock", (v: string) => TIME_FORMAT_LABEL[v] ?? v],
  liveStart: [
    "When a channel opens",
    (v: string) => LIVE_START_CHOICES.find((c) => c.value === v)?.label ?? v,
  ],
  simultaneousStreams: ["Simultaneous streams", (v: number) => String(v)],
  subtitlesOn: ["Subtitles", onOff],
  subtitleLanguage: ["Subtitle language", (v: string | null) => languageName(v) ?? "none"],
};

const SUBTITLE_LABELS: Record<keyof SubtitleStyleDto, [string, (v: never) => string]> = {
  font: ["Subtitle font", (v: string) => v],
  size: ["Subtitle size", (v: number) => `${v}pt`],
  color: ["Subtitle colour", (v: string) => v],
  backdrop: ["Subtitle backdrop", (v: string) => v],
  backdropOpacity: ["Subtitle backdrop opacity", (v: number) => `${Math.round(v * 100)}%`],
  outlineSize: ["Subtitle outline size", (v: number) => String(v)],
  shadowOffset: ["Subtitle shadow offset", (v: number) => String(v)],
};

const count = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

function listChange(label: string, from: readonly string[], to: readonly string[]): Change | null {
  const same = from.length === to.length && from.every((x, i) => x === to[i]);
  if (same) return null;
  const sameSet = from.length === to.length && from.every((x) => to.includes(x));
  return {
    label,
    from: count(from.length, "category", "categories"),
    to: sameSet
      ? `${count(to.length, "category", "categories")}, reordered`
      : from.length === to.length
        ? `${count(to.length, "category", "categories")}, different ones`
        : count(to.length, "category", "categories"),
  };
}

/** What importing `backup` over what is here would do, section by section. A
 *  section the file lacks is null; one that would change nothing has no
 *  changes, and the pane says so rather than offering to apply it. */
export function planImport(
  current: { settings: SettingsDto; categories: CategoryPrefs; activity: readonly ActivityRowDto[] },
  backup: ParsedBackup,
): ImportPlan {
  let settings: ImportPlan["settings"] = null;
  if (backup.settings) {
    const { subtitle, ...top } = backup.settings;
    const next = clampSettings({
      ...current.settings,
      ...top,
      subtitle: { ...current.settings.subtitle, ...subtitle },
    } as SettingsDto);
    const changes: Change[] = [];
    for (const key of Object.keys(SETTING_LABELS) as (keyof typeof SETTING_LABELS)[]) {
      if (next[key] !== current.settings[key]) {
        const [label, show] = SETTING_LABELS[key];
        const fmt = show as (v: unknown) => string;
        changes.push({ label, from: fmt(current.settings[key]), to: fmt(next[key]) });
      }
    }
    for (const key of Object.keys(SUBTITLE_LABELS) as (keyof SubtitleStyleDto)[]) {
      if (next.subtitle[key] !== current.settings.subtitle[key]) {
        const [label, show] = SUBTITLE_LABELS[key];
        const fmt = show as (v: unknown) => string;
        changes.push({
          label,
          from: fmt(current.settings.subtitle[key]),
          to: fmt(next.subtitle[key]),
        });
      }
    }
    settings = { next, changes };
  }

  let categories: ImportPlan["categories"] = null;
  if (backup.categories) {
    const next: CategoryPrefs = {
      pins: backup.categories.pins ?? [...current.categories.pins],
      hidden: backup.categories.hidden ?? [...current.categories.hidden],
      sort: backup.categories.sort ?? current.categories.sort,
    };
    const changes = [
      listChange("Pinned categories", current.categories.pins, next.pins),
      listChange("Hidden categories", current.categories.hidden, next.hidden),
      next.sort === current.categories.sort
        ? null
        : {
            label: "Category order",
            from: SORT_LABEL[current.categories.sort].long,
            to: SORT_LABEL[next.sort].long,
          },
    ].filter((c): c is Change => c !== null);
    categories = { next, changes };
  }

  let favourites: ImportPlan["favourites"] = null;
  if (backup.favourites) {
    const here = new Map(current.activity.map((r) => [r.streamId, r]));
    let added = 0;
    let raised = 0;
    for (const r of backup.favourites) {
      const local = here.get(r.streamId);
      if (!local) {
        if (r.watchSeconds > 0 || r.openCount > 0 || r.lastWatched !== null) added++;
        continue;
      }
      const merged = mergeActivity(local, r);
      if (
        merged.watchSeconds !== local.watchSeconds ||
        merged.lastWatched !== local.lastWatched ||
        merged.openCount !== local.openCount
      ) {
        raised++;
      }
    }
    favourites = { rows: backup.favourites, added, raised };
  }

  return { settings, categories, favourites };
}

/** Whether a section has anything to apply. */
export function planHasChanges(plan: ImportPlan): {
  settings: boolean;
  categories: boolean;
  favourites: boolean;
} {
  return {
    settings: (plan.settings?.changes.length ?? 0) > 0,
    categories: (plan.categories?.changes.length ?? 0) > 0,
    favourites: (plan.favourites?.added ?? 0) + (plan.favourites?.raised ?? 0) > 0,
  };
}
