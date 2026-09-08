// The command palette's list model. Channel matching is the backend's job —
// one nucleo index serves both the filter and the palette, and reimplementing
// fuzzy ranking in TypeScript would just make the two disagree. Only the small
// fixed action list is filtered here.
//
// Checked in src/__checks__/palette.test.mjs.

import type { ChannelDto } from "../types";

export interface PaletteAction {
  id: string;
  label: string;
  hint?: string;
  /** Actions that need something playing are hidden when nothing is. */
  needsPlayer?: boolean;
}

export type PaletteItem =
  | { kind: "action"; action: PaletteAction }
  | { kind: "channel"; channel: ChannelDto };

export const ACTIONS: PaletteAction[] = [
  { id: "compressor", label: "Toggle compressor", hint: "audio", needsPlayer: true },
  { id: "live", label: "Jump to live edge", hint: "L", needsPlayer: true },
  { id: "pause", label: "Play / pause", hint: "Space", needsPlayer: true },
  { id: "stop", label: "Stop playback", hint: "", needsPlayer: true },
  { id: "fullscreen", label: "Toggle fullscreen", hint: "F", needsPlayer: true },
  { id: "guide", label: "Open the guide", hint: "G" },
  { id: "sync", label: "Refresh data from the panel", hint: "sync" },
  { id: "settings", label: "Open settings", hint: "⚙" },
];

export function availableActions(playing: boolean): PaletteAction[] {
  return ACTIONS.filter((a) => !a.needsPlayer || playing);
}

/** Case-insensitive subsequence, the same shape of match the palette gives
 *  for channels, so typing "tc" finds "Toggle compressor". The hint is part of
 *  the haystack: "sync" is what someone types looking for "Refresh data from
 *  the provider", and the word is right there on the row. */
export function actionMatches(label: string, query: string, hint = ""): boolean {
  const q = query.trim().toLowerCase();
  if (q === "") return true;
  const h = `${label} ${hint}`.toLowerCase();
  let i = 0;
  for (let j = 0; j < h.length && i < q.length; j++) {
    if (h[j] === q[i]) i++;
  }
  return i === q.length;
}

export function filterActions(
  actions: PaletteAction[],
  query: string,
): PaletteAction[] {
  return actions.filter((a) => actionMatches(a.label, query, a.hint ?? ""));
}

/** Actions first: they are few, they are exact, and burying them under 100
 *  channel hits would make the palette useless as a command surface. */
export function paletteItems(
  actions: PaletteAction[],
  channels: ChannelDto[],
): PaletteItem[] {
  return [
    ...actions.map((action): PaletteItem => ({ kind: "action", action })),
    ...channels
      // Separators are never search hits, and never selectable here either.
      .filter((c) => !c.isSeparator)
      .map((channel): PaletteItem => ({ kind: "channel", channel })),
  ];
}

export function clampIndex(index: number, length: number): number {
  if (length <= 0) return 0;
  return Math.min(length - 1, Math.max(0, index));
}

/** Arrow-key movement wraps, because a palette with seven items and a long
 *  channel tail is quicker to reach from the bottom. */
export function moveIndex(index: number, delta: number, length: number): number {
  if (length <= 0) return 0;
  return (((index + delta) % length) + length) % length;
}
