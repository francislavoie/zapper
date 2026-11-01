// Settings the frontend has to know about before the backend answers.
//
// `settings()` is a round trip and it can fail — today it always does, since
// the command is not implemented yet. The guide still needs to know how many
// hours to draw, so there is a default here that matches the contract's
// documented defaults exactly (subtitles: background at 0.25), and the UI says
// plainly when it is showing these rather than the backend's.
//
// Pure; checked in src/__checks__/settings.test.mjs.

import type { SettingsDto, SubtitleStyleDto } from "../types";

export const DEFAULT_SUBTITLE: SubtitleStyleDto = {
  font: "Sans",
  size: 38,
  color: "#ffffff",
  backdrop: "background",
  backdropOpacity: 0.25,
  outlineSize: 2,
  shadowOffset: 0,
};

export const DEFAULT_SETTINGS: SettingsDto = {
  // "auto" follows the system. Only English ships today.
  language: "auto",
  rewindMb: 512,
  hardwareDecode: true,
  toneMapHdr: true,
  guideHours: 3,
  syncIntervalHours: 6,
  subtitle: DEFAULT_SUBTITLE,
  checkForUpdates: true,
  // Opt-in: the desktop gives the media keys to one player at a time.
  mediaControls: false,
  // The locale's clock until someone says otherwise.
  timeFormat: "auto",
};

/** Rewind buffer, in megabytes. The ceiling is the backend's: the buffer is
 *  resident memory, and two gigabytes is as much as it will take. */
export const REWIND_MIN = 64;
export const REWIND_MAX = 2000;
export const REWIND_STEP = 32;

/** Bitrates people will recognise, for saying what a buffer size buys. The
 *  1080p50 figure is typical of a live sports feed. */
export const REWIND_BITRATES: ReadonlyArray<{ label: string; mbps: number }> = [
  { label: "SD", mbps: 3 },
  { label: "720p", mbps: 6 },
  { label: "1080p", mbps: 10 },
  { label: "1080p50", mbps: 18 },
  { label: "4K", mbps: 25 },
];

/** Seconds of video `megabytes` holds at `mbps` megabits per second. The
 *  setting is bytes because that is all mpv's buffer understands; this is
 *  the honest way to say what that means in time — per bitrate, since the
 *  same buffer holds three times as long on an SD channel as on a 1080p one. */
export function rewindSeconds(megabytes: number, mbps: number): number {
  if (!(megabytes > 0) || !(mbps > 0)) return 0;
  return (megabytes * 8) / mbps;
}
export const SUB_SIZE_MIN = 12;
export const SUB_SIZE_MAX = 72;
export const SYNC_MIN = 1;
export const SYNC_MAX = 24;

/** The clock choices as Settings and a backup's import preview name them. */
export const TIME_FORMAT_LABEL: Record<string, string> = {
  auto: "System",
  "12h": "12-hour",
  "24h": "24-hour",
};

// ------------------------------------------------------- one setting's reset

/** A setting Settings can put back on its own: a top-level field, or one of
 *  the subtitle style's. */
export type SettingKey =
  | Exclude<keyof SettingsDto, "subtitle">
  | `subtitle.${keyof SubtitleStyleDto}`;

/** The family names that mean the default font. The backend's own default
 *  is fontconfig's "sans-serif" where this one says "Sans": the same face,
 *  so neither should offer to reset to the other. */
const DEFAULT_FONTS = new Set(["sans", "sans-serif"]);

function valueOf(s: SettingsDto, key: SettingKey): unknown {
  if (key.startsWith("subtitle.")) {
    return s.subtitle[key.slice("subtitle.".length) as keyof SubtitleStyleDto];
  }
  return s[key as Exclude<keyof SettingsDto, "subtitle">];
}

/** Whether `key` is at its default, which is when Settings hides its ↺. */
export function isDefault(s: SettingsDto, key: SettingKey): boolean {
  const value = valueOf(s, key);
  if (key === "subtitle.font") return DEFAULT_FONTS.has(String(value).trim().toLowerCase());
  return value === valueOf(DEFAULT_SETTINGS, key);
}

/** The patch that puts `key` back to its default and leaves the rest. */
export function resetPatch(s: SettingsDto, key: SettingKey): Partial<SettingsDto> {
  if (key.startsWith("subtitle.")) {
    const field = key.slice("subtitle.".length) as keyof SubtitleStyleDto;
    return { subtitle: { ...s.subtitle, [field]: DEFAULT_SUBTITLE[field] } };
  }
  const k = key as Exclude<keyof SettingsDto, "subtitle">;
  return { [k]: DEFAULT_SETTINGS[k] } as Partial<SettingsDto>;
}

const clamp = (n: number, lo: number, hi: number) =>
  Number.isFinite(n) ? Math.min(hi, Math.max(lo, n)) : lo;

const round = (n: number, lo: number, hi: number) => Math.round(clamp(n, lo, hi));

/** A colour the backend and libass will both accept. Anything else falls back
 *  to white rather than being passed on to mpv to choke on. */
export function normaliseColor(color: string): string {
  const c = color.trim().toLowerCase();
  if (/^#[0-9a-f]{6}$/.test(c)) return c;
  if (/^#[0-9a-f]{3}$/.test(c)) return `#${c[1]}${c[1]}${c[2]}${c[2]}${c[3]}${c[3]}`;
  return DEFAULT_SUBTITLE.color;
}

export function clampSubtitle(s: SubtitleStyleDto): SubtitleStyleDto {
  return {
    font: s.font.trim() === "" ? DEFAULT_SUBTITLE.font : s.font,
    size: round(s.size, SUB_SIZE_MIN, SUB_SIZE_MAX),
    color: normaliseColor(s.color),
    backdrop: s.backdrop,
    backdropOpacity: clamp(s.backdropOpacity, 0, 1),
    outlineSize: clamp(s.outlineSize, 0, 8),
    shadowOffset: clamp(s.shadowOffset, 0, 8),
  };
}

/** Everything that goes to the backend goes through here first: the sliders
 *  are bounded in the DOM too, but a stale stored value or a hand-edited
 *  config should not reach mpv unchecked. */
export function clampSettings(s: SettingsDto): SettingsDto {
  return {
    // Not clamped to a known list: an unknown tag falls back at lookup time,
    // which keeps a hand-edited config from being silently rewritten.
    language: s.language || "auto",
    rewindMb: round(s.rewindMb, REWIND_MIN, REWIND_MAX),
    hardwareDecode: !!s.hardwareDecode,
    toneMapHdr: !!s.toneMapHdr,
    guideHours: round(s.guideHours, 1, 6),
    syncIntervalHours: round(s.syncIntervalHours, SYNC_MIN, SYNC_MAX),
    subtitle: clampSubtitle(s.subtitle),
    // Only an explicit `false` turns it off: a config from before the setting
    // existed has no value, and that must read as the default, on.
    checkForUpdates: s.checkForUpdates !== false,
    // The other way round: only an explicit `true` takes the media keys.
    mediaControls: s.mediaControls === true,
    // A value this build does not know is the locale's clock, as missing is.
    timeFormat: s.timeFormat === "12h" || s.timeFormat === "24h" ? s.timeFormat : "auto",
  };
}

// ------------------------------------------------------------- the preview

/** CSS for the sample line in the subtitles pane.
 *
 *  libass draws the real subtitles into the video plane, which is beneath the
 *  webview: they cannot be seen from here, and nobody can judge a size or a
 *  backdrop from a number. So the preview is the control. It is CSS, not
 *  libass, so it is an approximation — but the *shape* of each backdrop mode
 *  is exactly what mpv will draw, and the size scales the same way. */
export interface SubtitlePreview {
  /** The full-width band behind the line, for the "window" backdrop only. */
  band: { background: string } | null;
  text: {
    color: string;
    fontFamily: string;
    fontSize: string;
    lineHeight: string;
    background: string;
    padding: string;
    textShadow: string;
  };
}

const FONT_STACKS: Record<string, string> = {
  Sans: "system-ui, sans-serif",
  Serif: "Georgia, 'Times New Roman', serif",
  Monospace: "ui-monospace, 'DejaVu Sans Mono', monospace",
};

/** mpv scales subtitles as if the window were 720 lines tall, so the same
 *  number has to be scaled by the preview's own height to look right. */
export function previewFontPx(size: number, frameHeight: number): number {
  return Math.max(6, (clampSubtitle({ ...DEFAULT_SUBTITLE, size }).size * frameHeight) / 720);
}

const black = (alpha: number) => `rgba(0, 0, 0, ${Math.round(alpha * 1000) / 1000})`;

/** An outline as eight offsets, which is what a text-stroke looks like when
 *  the engine has no text-stroke. */
function outline(px: number, color: string): string {
  if (px <= 0) return "none";
  const r = Math.round(px * 10) / 10;
  return [
    [-r, -r],
    [0, -r],
    [r, -r],
    [r, 0],
    [r, r],
    [0, r],
    [-r, r],
    [-r, 0],
  ]
    .map(([x, y]) => `${x}px ${y}px 0 ${color}`)
    .join(", ");
}

export function subtitlePreview(
  raw: SubtitleStyleDto,
  frameHeight: number,
): SubtitlePreview {
  const s = clampSubtitle(raw);
  const px = previewFontPx(s.size, frameHeight);
  const scale = frameHeight / 720;

  const text: SubtitlePreview["text"] = {
    color: s.color,
    fontFamily: FONT_STACKS[s.font] ?? `"${s.font}", system-ui, sans-serif`,
    fontSize: `${Math.round(px * 10) / 10}px`,
    lineHeight: "1.25",
    background: "transparent",
    padding: "0",
    textShadow: "none",
  };
  let band: SubtitlePreview["band"] = null;

  switch (s.backdrop) {
    case "background":
      text.background = black(s.backdropOpacity);
      text.padding = "0.1em 0.35em";
      break;
    case "window":
      band = { background: black(s.backdropOpacity) };
      break;
    case "outline":
      text.textShadow = outline(s.outlineSize * scale, "#000");
      break;
    case "shadow":
      text.textShadow =
        s.shadowOffset > 0
          ? `${Math.round(s.shadowOffset * scale * 10) / 10}px ${
              Math.round(s.shadowOffset * scale * 10) / 10
            }px 2px rgba(0, 0, 0, 0.85)`
          : "none";
      break;
    case "none":
      break;
  }

  return { band, text };
}

/** Which commands a change needs. Subtitle styling is a separate mpv call;
 *  the playback properties need applying as well as storing; the guide's two
 *  are ours alone and only have to be saved. */
export interface SavePlan {
  applyPlayback: boolean;
  applySubtitle: boolean;
}

const PLAYBACK_KEYS = ["rewindMb", "hardwareDecode", "toneMapHdr"] as const;

export function planSave(before: SettingsDto, after: SettingsDto): SavePlan {
  return {
    applyPlayback: PLAYBACK_KEYS.some((k) => before[k] !== after[k]),
    applySubtitle: (Object.keys(after.subtitle) as (keyof SubtitleStyleDto)[]).some(
      (k) => before.subtitle[k] !== after.subtitle[k],
    ),
  };
}
