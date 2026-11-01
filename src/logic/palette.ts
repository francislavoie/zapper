// The command palette's list model. Channel matching is the backend's job —
// one nucleo index serves both the filter and the palette, and reimplementing
// fuzzy ranking in TypeScript would just make the two disagree. Only the small
// fixed action list is filtered here.
//
// Checked in src/__checks__/palette.test.mjs.

import type { ChannelDto } from "../types";

/** Where an action sits when the list is drawn as a menu rather than filtered
 *  as a palette. The palette ignores this; the toolbar's hamburger rules a
 *  line between groups. */
export type ActionGroup = "view" | "player" | "data" | "app";

/** Every action the app has, named once. `runAction` in src/actions.ts takes a
 *  `Record<ActionId, …>`, so `tsc` fails the build if an action here has no
 *  behaviour, or a behaviour exists for an action nobody can reach. */
export const ACTION_IDS = [
  "browse",
  "guide",
  "compressor",
  "live",
  "pause",
  "stop",
  "fullscreen",
  "sync",
  "settings",
  "updates",
  "about",
] as const;

export type ActionId = (typeof ACTION_IDS)[number];

export interface PaletteAction {
  id: ActionId;
  label: string;
  hint?: string;
  group: ActionGroup;
  /** Actions that need something playing are hidden when nothing is. */
  needsPlayer?: boolean;
}

export type PaletteItem =
  | { kind: "action"; action: PaletteAction }
  | { kind: "channel"; channel: ChannelDto };

/** The one list. The command palette filters it; the toolbar's hamburger menu
 *  groups it. Adding an action here puts it in both, and `runAction` in
 *  src/actions.ts is required by a check to handle every id, so the surfaces
 *  cannot drift apart. */
export const ACTIONS: PaletteAction[] = [
  { id: "browse", label: "Browse channels", hint: "Esc", group: "view" },
  { id: "guide", label: "Open the guide", hint: "G", group: "view" },
  {
    id: "compressor",
    label: "Toggle compressor",
    hint: "audio",
    needsPlayer: true,
    group: "player",
  },
  { id: "live", label: "Jump to live edge", hint: "L", needsPlayer: true, group: "player" },
  { id: "pause", label: "Play / pause", hint: "Space", needsPlayer: true, group: "player" },
  { id: "stop", label: "Stop playback", hint: "", needsPlayer: true, group: "player" },
  {
    id: "fullscreen",
    label: "Toggle fullscreen",
    hint: "F",
    needsPlayer: true,
    group: "player",
  },
  { id: "sync", label: "Refresh data from the panel", hint: "sync", group: "data" },
  { id: "settings", label: "Open settings", hint: "⚙", group: "app" },
  // "upgrade" and "version" are what people type looking for this one.
  { id: "updates", label: "Check for updates", hint: "upgrade · version", group: "app" },
  { id: "about", label: "About zapper", hint: "", group: "app" },
];

/** The order the menu draws groups in, so a new group cannot land in an
 *  arbitrary place depending on where its first action happened to be
 *  declared. */
export const GROUP_ORDER: ActionGroup[] = ["view", "player", "data", "app"];

/** The palette's own action list, cut into the menu's groups, empty groups
 *  dropped. The menu is therefore a rearrangement of the palette and never a
 *  second list: it cannot hold an action the palette lacks, nor miss one it
 *  has. */
export function actionGroups(actions: PaletteAction[]): PaletteAction[][] {
  return GROUP_ORDER.map((g) => actions.filter((a) => a.group === g)).filter(
    (g) => g.length > 0,
  );
}

/** The menu's items flattened back into one list, which is the order the
 *  keyboard walks them in. */
export function flatGroups(groups: PaletteAction[][]): PaletteAction[] {
  return groups.flat();
}

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
