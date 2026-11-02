// The command palette's list model. Channel matching is the backend's job —
// one nucleo index serves both the filter and the palette, and reimplementing
// fuzzy ranking in TypeScript would just make the two disagree — and so is
// programme matching, from the guide's title index. Only the small fixed
// action list is filtered here.
//
// Checked in src/__checks__/palette.test.mjs.

import type { ChannelDto, ProgrammeHitDto } from "../types";
import { allows, DESKTOP, type Capabilities, type Capability } from "./capabilities.ts";
import { anyAlternative } from "./searchQuery.ts";
import type { SleepRequest } from "./sleep";

/** Where an action sits when the list is drawn as a menu rather than filtered
 *  as a palette. The palette ignores this; the toolbar's hamburger rules a
 *  line between groups. */
export type ActionGroup = "view" | "player" | "sleep" | "data" | "app";

/** Every action the app has, named once. `runAction` in src/actions.ts takes a
 *  `Record<ActionId, …>`, so `tsc` fails the build if an action here has no
 *  behaviour, or a behaviour exists for an action nobody can reach. */
export const ACTION_IDS = [
  "browse",
  "guide",
  "previous",
  "compressor",
  "live",
  "pause",
  "stop",
  "fullscreen",
  "theater",
  "stats",
  "sleep15",
  "sleep30",
  "sleep45",
  "sleep60",
  "sleep90",
  "sleepProgramme",
  "sleepOff",
  "sync",
  "record",
  "recordings",
  "settings",
  "updates",
  "about",
  "quit",
  "signOut",
] as const;

export type ActionId = (typeof ACTION_IDS)[number];

export interface PaletteAction {
  id: ActionId;
  label: string;
  hint?: string;
  group: ActionGroup;
  /** Actions that need something playing are hidden when nothing is. */
  needsPlayer?: boolean;
  /** Hidden until there is a previous channel to go back to. */
  needsPrevious?: boolean;
  /** Hidden unless the condition holds: a sleep timer is set, the guide
   *  knows when the programme on now ends, or it can be recorded. */
  needs?: "sleeping" | "programme" | "recordable";
  /** Hidden where the interface cannot do it (logic/capabilities.ts). */
  needsCapability?: Capability;
  /** A sleep timer action: what it sets, or `null` for off. */
  sleep?: SleepRequest | null;
  /** What it is called inside a submenu that already says what it is part
   *  of: "30 minutes" under "Sleep timer". */
  short?: string;
}

/** What `needsPrevious`, `needs` and `needsCapability` are checked against. */
export interface ActionConditions {
  hasPrevious?: boolean;
  sleeping?: boolean;
  programme?: boolean;
  /** The programme on now can be recorded (logic/recordings.ts
   *  `playerRecord`). */
  recordable?: boolean;
  /** The app's window when not given. */
  capabilities?: Capabilities;
}

export type PaletteItem =
  | { kind: "action"; action: PaletteAction }
  | { kind: "channel"; channel: ChannelDto }
  | { kind: "programme"; hit: ProgrammeHitDto };

/** The one list. The command palette filters it; the toolbar's hamburger menu
 *  groups it. Adding an action here puts it in both, and `runAction` in
 *  src/actions.ts is required by a check to handle every id, so the surfaces
 *  cannot drift apart. */
export const ACTIONS: PaletteAction[] = [
  { id: "browse", label: "Browse channels", hint: "Esc", group: "view" },
  { id: "guide", label: "Open the guide", hint: "G", group: "view" },
  // A remote's recall button. Not tied to something playing: after a stop,
  // going back to what was on is still a reasonable thing to want.
  // "last" and "recall" are what the button is called on remotes.
  {
    id: "previous",
    label: "Previous channel",
    hint: "P · last · recall",
    needsPrevious: true,
    needsCapability: "playback",
    group: "player",
  },
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
  // T toggles; from here it opens, since a menu entry has no "back" half.
  {
    id: "theater",
    label: "Theater: picture fills the window",
    hint: "T",
    needsPlayer: true,
    group: "player",
  },
  // A view, not a player control: it opens with nothing playing too.
  { id: "stats", label: "Stats for nerds", hint: "S", group: "view", needsCapability: "mpv" },
  // Each length its own entry, so typing "sleep 30" is one Enter away. The
  // hamburger folds the group into one row with a submenu.
  ...([15, 30, 45, 60, 90] as const).map(
    (m): PaletteAction => ({
      id: `sleep${m}`,
      label: `Sleep timer: ${m} minutes`,
      short: `${m} minutes`,
      group: "sleep",
      needsPlayer: true,
      sleep: m,
    }),
  ),
  {
    id: "sleepProgramme",
    label: "Sleep timer: end of this programme",
    short: "End of this programme",
    group: "sleep",
    needsPlayer: true,
    needs: "programme",
    sleep: "programme",
  },
  {
    id: "sleepOff",
    label: "Sleep timer: off",
    short: "Off",
    group: "sleep",
    needsPlayer: true,
    needs: "sleeping",
    sleep: null,
  },
  {
    id: "sync",
    label: "Refresh data from the provider",
    hint: "sync",
    group: "data",
    needsCapability: "resync",
  },
  // The programme on the channel playing, from the player: R.
  {
    id: "record",
    label: "Record this programme",
    hint: "R",
    needsPlayer: true,
    needs: "recordable",
    group: "player",
  },
  // What the recording service has scheduled and recorded. "record" and
  // "dvr" are what people type looking for it.
  { id: "recordings", label: "Recordings", hint: "record · dvr", group: "data" },
  { id: "settings", label: "Open settings", hint: "⚙", group: "app" },
  // "upgrade" and "version" are what people type looking for this one.
  {
    id: "updates",
    label: "Check for updates",
    hint: "upgrade · version",
    group: "app",
    needsCapability: "updater",
  },
  { id: "about", label: "About zapper", hint: "", group: "app" },
  { id: "quit", label: "Quit zapper", hint: "Ctrl+Q", group: "app", needsCapability: "window" },
  // In a browser signed in to the recording service: forgets this browser's
  // sign-in there, as revoking it in the app's Settings would.
  {
    id: "signOut",
    label: "Sign out",
    hint: "log out",
    group: "app",
    needsCapability: "signOut",
  },
];

/** The order the menu draws groups in, so a new group cannot land in an
 *  arbitrary place depending on where its first action happened to be
 *  declared. */
export const GROUP_ORDER: ActionGroup[] = ["view", "player", "sleep", "data", "app"];

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

export function availableActions(
  playing: boolean,
  conditions: ActionConditions = {},
): PaletteAction[] {
  const caps = conditions.capabilities ?? DESKTOP;
  return ACTIONS.filter(
    (a) =>
      allows(caps, a.needsCapability) &&
      (!a.needsPlayer || (playing && caps.playback)) &&
      (!a.needsPrevious || conditions.hasPrevious === true) &&
      (a.needs === undefined || conditions[a.needs] === true),
  );
}

/** Case-insensitive subsequence, the same shape of match the palette gives
 *  for channels, so typing "tc" finds "Toggle compressor". The hint is part of
 *  the haystack: "sync" is what someone types looking for "Refresh data from
 *  the provider", and the word is right there on the row. */
export function actionMatches(label: string, query: string, hint = ""): boolean {
  const h = `${label} ${hint}`.toLowerCase();
  // Alternatives (`sleep|stop`) as the channel search reads them.
  return anyAlternative(query, (alt) => {
    const q = alt.toLowerCase();
    let i = 0;
    for (let j = 0; j < h.length && i < q.length; j++) {
      if (h[j] === q[i]) i++;
    }
    return i === q.length;
  });
}

export function filterActions(
  actions: PaletteAction[],
  query: string,
): PaletteAction[] {
  return actions.filter((a) => actionMatches(a.label, query, a.hint ?? ""));
}

/** Actions first: they are few, they are exact, and burying them under 100
 *  channel hits would make the palette useless as a command surface. Then
 *  channels, then programmes, each in the backend's order. */
export function paletteItems(
  actions: PaletteAction[],
  channels: ChannelDto[],
  programmes: ProgrammeHitDto[] = [],
): PaletteItem[] {
  return [
    ...actions.map((action): PaletteItem => ({ kind: "action", action })),
    ...channels
      // Separators are never search hits, and never selectable here either.
      .filter((c) => !c.isSeparator)
      .map((channel): PaletteItem => ({ kind: "channel", channel })),
    ...programmes.map((hit): PaletteItem => ({ kind: "programme", hit })),
  ];
}

/** The heading drawn above item `i`, where a group of search results begins.
 *  Actions lead the list and need none; channels and programmes are labelled
 *  where they start, since a programme row and a channel row can carry the
 *  same name. */
export function groupHeading(items: PaletteItem[], i: number): string | null {
  const kind = items[i]?.kind;
  if (kind === undefined || kind === "action" || items[i - 1]?.kind === kind) return null;
  return kind === "channel" ? "Channels" : "Programmes";
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
