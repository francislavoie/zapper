// Subtitles on or off, as a standing choice rather than one stream's.
//
// The settings keep whether subtitles are on (`subtitlesOn`) and the
// language of the last track the viewer chose (`subtitleLanguage`). Every
// open applies them once its tracks are known: off is none; on is the first
// track in that language, else the first track. Choosing in the picture's
// menu, the transport's toggle or with C is what changes them. The app's
// player makes the same choice in Rust (`subtitle_choice` in
// src-tauri/src/ipc/player_cmds.rs); the browser's and the mock's make it
// here.
//
// Pure. Checked in src/__checks__/subtitles.test.mjs.

import type { SettingsDto, TrackDto } from "../types";
import { isPictureSubtitle } from "./tracks.ts";

export type SubtitlePreference = Pick<SettingsDto, "subtitlesOn" | "subtitleLanguage">;

/** What a track needs for the choice: its language, and whether it is
 *  forced (only the lines in another language, or the signs). */
export interface SubtitleLike {
  lang: string | null;
  forced?: boolean;
}

const sameLanguage = (a: string | null, b: string | null): boolean =>
  a !== null && b !== null && a.toLowerCase() === b.toLowerCase();

/** The place in `tracks` of the one turning subtitles on shows: the first in
 *  `language`, else the first — a full track before a forced one either
 *  way. Only those `usable` says can be shown are considered; `null` when
 *  none can. */
export function preferredIndex<T extends SubtitleLike>(
  tracks: readonly T[],
  language: string | null,
  usable: (t: T) => boolean = () => true,
): number | null {
  const tiers: ((t: T) => boolean)[] = [
    (t) => sameLanguage(t.lang, language) && !t.forced,
    (t) => sameLanguage(t.lang, language),
    (t) => !t.forced,
    () => true,
  ];
  for (const tier of tiers) {
    const i = tracks.findIndex((t) => usable(t) && tier(t));
    if (i >= 0) return i;
  }
  return null;
}

/** Whether a track can be shown where pictures can (`pictures`) or can't. */
const showable = (pictures: boolean) => (t: Pick<TrackDto, "codec">) =>
  pictures || !isPictureSubtitle(t);

/** The subtitle track an open shows under `pref`: its id, `null` for none,
 *  or `undefined` when it has nothing that could be shown (nothing to do). */
export function openChoice(
  pref: SubtitlePreference,
  tracks: readonly TrackDto[],
  pictures: boolean,
): number | null | undefined {
  if (!hasSubtitles(tracks, pictures)) return undefined;
  if (!pref.subtitlesOn) return null;
  const i = preferredIndex(tracks, pref.subtitleLanguage, showable(pictures));
  return i === null ? null : tracks[i].id;
}

/** What the toggle does: shown subtitles go off (`null`), hidden ones come
 *  on (the id to show, chosen as an open would). `undefined` when there is
 *  nothing to toggle. */
export function toggleChoice(
  tracks: readonly TrackDto[],
  language: string | null,
  pictures: boolean,
): number | null | undefined {
  if (!hasSubtitles(tracks, pictures)) return undefined;
  if (subtitlesShowing(tracks)) return null;
  const i = preferredIndex(tracks, language, showable(pictures));
  return i === null ? undefined : tracks[i].id;
}

/** The standing choice after the viewer chose `track`, or Off with `null`:
 *  on, in its language (the one there was, when it names none); off keeps
 *  the language, for the next time they are turned on. */
export function afterChoosing(
  track: Pick<TrackDto, "lang"> | null,
  pref: SubtitlePreference,
): SubtitlePreference {
  if (track === null) return { subtitlesOn: false, subtitleLanguage: pref.subtitleLanguage };
  return { subtitlesOn: true, subtitleLanguage: track.lang ? track.lang : pref.subtitleLanguage };
}

/** Whether the stream has subtitles that can be shown here: what the
 *  toggle's being there at all depends on. */
export function hasSubtitles(tracks: readonly Pick<TrackDto, "codec">[], pictures: boolean): boolean {
  return tracks.some(showable(pictures));
}

/** Whether subtitles are showing: the toggle lit. */
export function subtitlesShowing(tracks: readonly Pick<TrackDto, "selected">[]): boolean {
  return tracks.some((t) => t.selected);
}

/** The toggle's tooltip: what pressing it does. */
export function subtitleTip(showing: boolean): string {
  return showing ? "Subtitles off (C)" : "Subtitles (C)";
}
