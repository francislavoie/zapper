// What recordings are called on disk, for Settings › Recording's preview
// and its check of a custom pattern before it is sent.
//
// The recording service decides the real names
// (crates/zapper-core/src/recording/naming.rs) and refuses a pattern it cannot use;
// this is the same logic, rule for rule, so what the preview shows is what
// the service writes. Both are checked against
// crates/zapper-core/src/recording/naming_cases.json, and this one in
// src/__checks__/naming.test.mjs.
//
// A pattern is text with tokens such as {show} and {date}; `/` (or `\`)
// makes a folder. A token with no value takes the separators before it
// along — or after it, at the start of a name — and brackets right around
// it, so "{show} - {se} - {date}" is "News - 2026-11-03" for a programme
// without an episode number. Every name is made safe for Windows and Samba
// shares, and nothing can lead outside the output folder.

import type { Naming } from "../types";

export const MEDIA_SERVER = "{show}/{show} - {se} - {date} {time}";

export interface NamingPreset {
  id: Naming;
  label: string;
  tip: string;
  /** Empty for "custom". */
  pattern: string;
}

export const NAMING_PRESETS: readonly NamingPreset[] = [
  {
    id: "mediaServer",
    label: "Media server",
    tip: "For Jellyfin, Plex and Emby: a folder per show",
    pattern: MEDIA_SERVER,
  },
  {
    id: "byChannel",
    label: "By channel",
    tip: "A folder per channel",
    pattern: "{channel}/{show} - {date} {time}",
  },
  {
    id: "byDate",
    label: "By date",
    tip: "A folder per day",
    pattern: "{date}/{time} {channel} - {show}",
  },
  {
    id: "flat",
    label: "Flat",
    tip: "Every recording straight in the folder",
    pattern: "{show} - {channel} - {date} {time}",
  },
  { id: "custom", label: "Custom", tip: "Your own pattern", pattern: "" },
];

export const NAMING_DEFAULT: Naming = "mediaServer";

export function presetLabel(n: Naming): string {
  return NAMING_PRESETS.find((p) => p.id === n)?.label ?? n;
}

/** The pattern recordings are named by; a custom one that is not valid is
 *  read as the default, as the service does. */
export function patternFor(naming: Naming, custom: string): string {
  const preset = NAMING_PRESETS.find((p) => p.id === naming);
  if (preset && preset.pattern) return preset.pattern;
  return validatePattern(custom) === null ? custom.trim() : MEDIA_SERVER;
}

export const TOKENS = [
  "show",
  "title",
  "channel",
  "date",
  "time",
  "year",
  "season",
  "episode",
  "se",
] as const;
export type Token = (typeof TOKENS)[number];

/** What each token gives, for the list under the custom pattern. */
export const TOKEN_HELP: Record<Token, string> = {
  show: "the programme",
  title: "the episode's title",
  channel: "the channel",
  date: "2026-11-03",
  time: "20.00, this computer's time",
  year: "2026",
  season: "02",
  episode: "04",
  se: "S02E04, or E04",
};

export const PATTERN_MAX = 250;
const VALUE_MAX = 100;
const SEGMENT_MAX = 180;
const GAP = "\0";
const FALLBACK_FILE = "{show} - {date} {time}";

/** A wall-clock reading, in the service's time zone. */
export interface Civil {
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
}

export interface NamingProgramme {
  title: string;
  episodeTitle?: string | null;
  channel?: string;
  season?: number | null;
  episode?: number | null;
}

export interface Place {
  /** Outermost first; none for a flat layout. */
  folders: string[];
  /** Without part number or extension. */
  base: string;
}

type Piece = { text: string } | { token: string };

const isWs = (c: string) => /\s/u.test(c);
const isCtrl = (c: string) => /\p{Cc}/u.test(c);
const isSeparator = (c: string) => isWs(c) || "-_.,–—·".includes(c);

function segments(pattern: string): Piece[][] {
  const segs: Piece[][] = [[]];
  const text = (t: string) =>
    t.split(/[/\\]/).forEach((part, i) => {
      if (i > 0) segs.push([]);
      if (part) segs[segs.length - 1].push({ text: part });
    });
  let rest = pattern;
  for (;;) {
    const i = rest.search(/[{}]/);
    if (i < 0) break;
    if (rest[i] === "}") throw new Error("a } has no { before it");
    text(rest.slice(0, i));
    const after = rest.slice(i + 1);
    const j = after.search(/[{}]/);
    if (j < 0 || after[j] !== "}") throw new Error("a { is not closed with }");
    const name = after.slice(0, j);
    if (!(TOKENS as readonly string[]).includes(name)) {
      throw new Error(
        `{${name}} is not a token; the tokens are ${TOKENS.map((t) => `{${t}}`).join(" ")}`,
      );
    }
    segs[segs.length - 1].push({ token: name });
    rest = after.slice(j + 1);
  }
  text(rest);
  return segs;
}

/** Why a pattern cannot be used, or null when it can: word for word what
 *  the service says. */
export function validatePattern(pattern: string): string | null {
  const p = pattern.trim();
  if (!p) return "the pattern is empty";
  if (Array.from(p).length > PATTERN_MAX) {
    return `the pattern is longer than ${PATTERN_MAX} characters`;
  }
  if (/^[/\\]/.test(p)) {
    return "the pattern cannot start with /: it names a place inside the output folder";
  }
  if (/^[A-Za-z]:/.test(p)) {
    return "the pattern cannot start with a drive: it names a place inside the output folder";
  }
  if (Array.from(p).some(isCtrl)) return "the pattern cannot contain control characters";
  const bad = Array.from(p).find((c) => '<>:"|?*'.includes(c));
  if (bad) return `names cannot contain ${bad} (Windows does not allow it)`;
  let segs: Piece[][];
  try {
    segs = segments(p);
  } catch (e) {
    return (e as Error).message;
  }
  for (const seg of segs) {
    if (seg.some((x) => "token" in x)) continue;
    const fixed = seg
      .map((x) => ("text" in x ? x.text : ""))
      .join("")
      .trim();
    if (!fixed) return "a folder name is empty (two / in a row, or one at the end)";
    if (/^\.+$/.test(fixed)) {
      return `“${fixed}” cannot be a folder: recordings stay inside the output folder`;
    }
    if (reserved(stem(fixed))) return `“${fixed}” is a reserved name on Windows`;
  }
  if (!segs[segs.length - 1].some((x) => "token" in x)) {
    return "the file name needs a token, such as {date} {time}, or every recording gets the same name";
  }
  return null;
}

const pad = (n: number, w = 2) => String(n).padStart(w, "0");

/** Where a programme starting at `start` goes under `pattern`. */
export function place(pattern: string, p: NamingProgramme, start: Civil): Place {
  const segs = segments(validatePattern(pattern) === null ? pattern.trim() : MEDIA_SERVER);
  const value = (token: string): string => {
    switch (token) {
      case "show":
        return clean(p.title, VALUE_MAX) || "Recording";
      case "title":
        return clean(p.episodeTitle ?? "", VALUE_MAX);
      case "channel":
        return clean(p.channel ?? "", VALUE_MAX);
      case "date":
        return `${pad(start.year, 4)}-${pad(start.month)}-${pad(start.day)}`;
      case "time":
        return `${pad(start.hour)}.${pad(start.minute)}`;
      case "year":
        return pad(start.year, 4);
      case "season":
        return p.season != null ? pad(p.season) : "";
      case "episode":
        return p.episode != null ? pad(p.episode) : "";
      case "se":
        if (p.episode == null) return "";
        return p.season != null ? `S${pad(p.season)}E${pad(p.episode)}` : `E${pad(p.episode)}`;
      default:
        return "";
    }
  };
  const names = segs.map((seg) => segment(seg, value));
  let base = names.pop() ?? "";
  if (!base) base = segment(segments(FALLBACK_FILE)[0], value);
  return { folders: names.filter(Boolean), base };
}

/** `folder/folder/base`. */
export const placePath = (pl: Place) => [...pl.folders, pl.base].join("/");

function segment(pieces: Piece[], value: (t: string) => string): string {
  const s: string[] = [];
  for (const piece of pieces) {
    if ("text" in piece) {
      for (const c of piece.text) s.push(isCtrl(c) ? " " : c);
    } else {
      const v = value(piece.token);
      if (v) s.push(...Array.from(v));
      else s.push(GAP);
    }
  }
  collapse(s);
  return clean(s.join(""), SEGMENT_MAX);
}

/** The service's rule (naming.rs `collapse`): brackets right around an
 *  empty token go; then the separators before it when there is text before
 *  it, else the ones after it. Leftmost first. */
function collapse(s: string[]): void {
  for (let i = 0; i < s.length; ) {
    if (
      s[i] === GAP &&
      i > 0 &&
      i + 1 < s.length &&
      ((s[i - 1] === "(" && s[i + 1] === ")") || (s[i - 1] === "[" && s[i + 1] === "]"))
    ) {
      s.splice(i + 1, 1);
      s.splice(i - 1, 1);
      i -= 1;
      continue;
    }
    i += 1;
  }
  for (let i = s.indexOf(GAP); i >= 0; i = s.indexOf(GAP)) {
    let j = i;
    while (j > 0 && isSeparator(s[j - 1])) j--;
    if (j > 0) {
      s.splice(j, i - j + 1);
    } else {
      let k = i + 1;
      while (k < s.length && isSeparator(s[k])) k++;
      s.splice(i, k - i);
    }
  }
}

const utf8 = new TextEncoder();

/** A name Windows, Samba and Linux all accept, or empty; at most `max`
 *  bytes, cut between characters. */
export function clean(name: string, max: number): string {
  let out = "";
  let space = false;
  const cs = Array.from(name);
  for (let i = 0; i < cs.length; i++) {
    let c = cs[i];
    if ('<>"|?*'.includes(c)) continue;
    if (c === ":" && i + 1 < cs.length && isWs(cs[i + 1])) {
      space = out !== "";
      c = "-";
    } else if (c === ":" || c === "/" || c === "\\") {
      c = "-";
    } else if (isCtrl(c)) {
      c = " ";
    }
    if (isWs(c)) {
      space = out !== "";
      continue;
    }
    if (space) {
      out += " ";
      space = false;
    }
    out += c;
  }
  out = out.replace(/^[. ]+/, "");
  if (utf8.encode(out).length > max) {
    let cut = "";
    let bytes = 0;
    for (const c of out) {
      const n = utf8.encode(c).length;
      if (bytes + n > max) break;
      cut += c;
      bytes += n;
    }
    out = cut;
  }
  out = out.replace(/[. ]+$/, "");
  const st = stem(out);
  if (reserved(st)) out = `${st}_${out.slice(st.length)}`;
  return out;
}

const stem = (name: string) => name.split(".")[0].trimEnd();

function reserved(stem: string): boolean {
  const u = stem.replace(/[a-z]/g, (c) => c.toUpperCase());
  return /^(CON|PRN|AUX|NUL|(COM|LPT)[0-9¹²³])$/u.test(u);
}

/** Examples the preview names: one the guide gives season and episode for,
 *  one it does not. */
export const NAMING_EXAMPLES: readonly { programme: NamingProgramme; start: Civil }[] = [
  {
    programme: {
      title: "Harbour Lights",
      episodeTitle: "The Lamp Keeper",
      channel: "Northwind One HD",
      season: 2,
      episode: 4,
    },
    start: { year: 2026, month: 11, day: 3, hour: 20, minute: 0 },
  },
  {
    programme: { title: "The Evening News", channel: "Meridian 24" },
    start: { year: 2026, month: 11, day: 4, hour: 18, minute: 0 },
  },
];

/** What starts a recording's `.strm` file name: square brackets, since
 *  Windows does not allow angle ones. */
export const STRM_MARK = "[RECORDING] ";

/** The `.strm` file that stands for a recording named `base` while it
 *  records, in Jellyfin (.strm) mode: `[RECORDING] <base>.strm`. */
export function strmFile(base: string): string {
  return `${STRM_MARK}${base}.strm`;
}

/** The full path a programme would be recorded to, with the folder's own
 *  separator (a Windows folder gets `\`). */
export function previewPath(
  outputDir: string,
  pattern: string,
  programme: NamingProgramme,
  start: Civil,
): string {
  const sep = outputDir.includes("\\") && !outputDir.includes("/") ? "\\" : "/";
  const pl = place(pattern, programme, start);
  const dir = outputDir.replace(/[/\\]+$/, "");
  return [dir, ...pl.folders, `${pl.base}.mkv`].join(sep);
}
