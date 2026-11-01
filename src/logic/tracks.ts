// What the picture menu says about a stream's tracks, and the fixed choices
// beside them (aspect, deinterlacing).
//
// mpv describes a track in codes — `fre`, `ac3`, six channels — and a menu
// of codes makes the viewer do the translation. The language comes first,
// because it is what anybody choosing an audio track is choosing between;
// everything else is detail, there to tell apart two tracks in the same
// language.
//
// Pure. Checked in src/__checks__/tracks.test.mjs.

import type { AspectMode, DeinterlaceMode, PlayerOptionsDto, TrackDto } from "../types";

/** ISO 639-2 has two codes for some twenty languages, and broadcasters use
 *  the bibliographic one (`fre`, `ger`) about as often as the terminology
 *  one (`fra`, `deu`). Only the latter is a valid language tag, so the
 *  former is translated before it is looked up. */
const BIBLIOGRAPHIC: Record<string, string> = {
  alb: "sq",
  arm: "hy",
  baq: "eu",
  bur: "my",
  chi: "zh",
  cze: "cs",
  dut: "nl",
  fre: "fr",
  geo: "ka",
  ger: "de",
  gre: "el",
  ice: "is",
  mac: "mk",
  mao: "mi",
  may: "ms",
  per: "fa",
  rum: "ro",
  slo: "sk",
  tib: "bo",
  wel: "cy",
};

let names: Intl.DisplayNames | null | undefined;

/** "French" for `fre`, `fra` or `fr`. The code itself, upper-cased, when
 *  nothing knows it — still something to choose by. */
export function languageName(code: string | null): string | null {
  const c = code?.trim().toLowerCase();
  if (!c) return null;
  const tag = BIBLIOGRAPHIC[c] ?? c;
  if (names === undefined) {
    try {
      names = new Intl.DisplayNames(["en"], { type: "language", fallback: "none" });
    } catch {
      names = null;
    }
  }
  let name: string | undefined;
  try {
    name = names?.of(tag);
  } catch {
    // Not a well-formed tag: a broadcaster's own code, `qaa`-style.
  }
  return name && name.toLowerCase() !== tag ? name : c.toUpperCase();
}

const CODECS: Record<string, string> = {
  aac: "AAC",
  aac_latm: "AAC",
  ac3: "Dolby Digital",
  eac3: "Dolby Digital Plus",
  dts: "DTS",
  mp2: "MPEG audio",
  mp3: "MP3",
  opus: "Opus",
  flac: "FLAC",
  truehd: "Dolby TrueHD",
  dvb_subtitle: "DVB",
  dvb_teletext: "Teletext",
  eia_608: "Closed captions",
  subrip: "Text",
  ass: "Text",
  webvtt: "Text",
  mov_text: "Text",
  hdmv_pgs_subtitle: "PGS",
};

export function codecName(codec: string | null): string | null {
  if (!codec) return null;
  return CODECS[codec] ?? codec.toUpperCase();
}

/** "stereo", "5.1". Nothing for a count that is not a familiar layout. */
export function channelLayout(n: number | null): string | null {
  switch (n) {
    case 1:
      return "mono";
    case 2:
      return "stereo";
    case 6:
      return "5.1";
    case 8:
      return "7.1";
    default:
      return n != null && n > 0 ? `${n} ch` : null;
  }
}

export interface TrackLabel {
  /** What it is: the language, else its title, else its place in the list. */
  label: string;
  /** What tells it apart from its neighbours, small and to the right. */
  detail: string;
}

/** `index` is the track's place in its list, from zero; it names a track
 *  that says nothing else about itself. */
export function trackLabel(t: TrackDto, index: number): TrackLabel {
  const language = languageName(t.lang);
  const label = language ?? t.title ?? `Track ${index + 1}`;
  const detail = [
    language && t.title ? t.title : null,
    t.visualImpaired ? "Audio description" : null,
    t.hearingImpaired ? "SDH" : null,
    t.forced ? "Forced" : null,
    codecName(t.codec),
    channelLayout(t.channels),
  ].filter((x): x is string => x !== null && x !== "");
  return { label, detail: detail.join(" · ") };
}

/** Whether there is an audio choice to make. One track is no choice, and the
 *  transport's audio button and the menu's submenu are absent for it. */
export function hasAudioChoice(o: Pick<PlayerOptionsDto, "audio">): boolean {
  return o.audio.length > 1;
}

/** The selected track's label, for the row that opens the submenu. */
export function selectedLabel(list: TrackDto[]): string | null {
  const i = list.findIndex((t) => t.selected);
  return i < 0 ? null : trackLabel(list[i], i).label;
}

export const ASPECT_CHOICES: readonly { value: AspectMode; label: string }[] = [
  { value: "auto", label: "Auto" },
  { value: "16:9", label: "16:9" },
  { value: "4:3", label: "4:3" },
  { value: "2.35:1", label: "2.35:1" },
  { value: "fill", label: "Fill (crop)" },
  { value: "stretch", label: "Stretch" },
];

export const DEINTERLACE_CHOICES: readonly { value: DeinterlaceMode; label: string }[] = [
  { value: "auto", label: "Auto" },
  { value: "on", label: "On" },
  { value: "off", label: "Off" },
];

export function choiceLabel<T extends string>(
  choices: readonly { value: T; label: string }[],
  value: T,
): string {
  return choices.find((c) => c.value === value)?.label ?? value;
}
