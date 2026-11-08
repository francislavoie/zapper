// Every keyboard shortcut and mouse gesture, as data the settings pane draws.
//
// Kept as data, beside the handlers rather than inside the pane's markup,
// because a list written as prose drifts from the keys it describes.
// src/__checks__/shortcuts.test.mjs checks the keys listed here
// against the handler's source, so a key that stops working leaves the list.
//
// Pure; no React.

import type { Capabilities, Capability } from "./capabilities.ts";

/** One way to press it: the keys of a chord, held together. "Ctrl" is the
 *  shortcut modifier, drawn as ⌘ on a Mac, and "Alt" and "Shift" as ⌥ and ⇧
 *  (logic/keys.ts `chordKeys`). */
export type Chord = readonly string[];

export interface Shortcut {
  /** Alternatives, any of which does it: [["←"], ["→"]] reads "← or →". */
  keys: readonly Chord[];
  /** Joined with "/" rather than "or" — a pair that does opposite things,
   *  like back and forward. */
  pair?: boolean;
  /** What it does, in a few words. */
  does: string;
  /** A mouse gesture rather than a key; drawn without keycaps. */
  mouse?: boolean;
  /** What it needs to do anything (logic/capabilities.ts); listed only
   *  where that is there. */
  needs?: Capability;
}

export interface ShortcutGroup {
  title: string;
  /** When the group applies, said once instead of on every row. */
  when?: string;
  /** What every item in it needs, said once. */
  needs?: Capability;
  items: readonly Shortcut[];
}

export const SHORTCUT_GROUPS: readonly ShortcutGroup[] = [
  {
    title: "Anywhere",
    items: [
      { keys: [["Ctrl", "P"]], does: "Command palette — every action, and channel search" },
      { keys: [["Ctrl", "Q"]], does: "Quit zapper", needs: "window" },
      { keys: [["/"]], does: "Search channels" },
      { keys: [["M"]], does: "Go to Movies", needs: "onDemand" },
      { keys: [["N"]], does: "Go to Shows", needs: "onDemand" },
      { keys: [["W"]], does: "Continue watching", needs: "onDemand" },
      { keys: [["G"]], does: "Switch between the list and the guide" },
      { keys: [["S"]], does: "Stats for nerds", needs: "mpv" },
      { keys: [["P"]], does: "Previous channel; again to come back", needs: "playback" },
      { keys: [["F"]], does: "Fullscreen" },
      { keys: [["Esc"]], does: "Back one step: a menu, a dialog, fullscreen, the player" },
      {
        keys: [["Ctrl", "+"], ["Ctrl", "−"]],
        pair: true,
        does: "Make everything bigger or smaller",
      },
      { keys: [["Ctrl", "0"]], does: "Back to normal size" },
    ],
  },
  {
    title: "While watching",
    when: "with a channel playing",
    needs: "playback",
    items: [
      { keys: [["Space"]], does: "Play or pause" },
      { keys: [["←"], ["→"]], pair: true, does: "Back or forward 5 seconds (10 in a movie or episode)" },
      { keys: [["L"]], does: "Jump to live (not in a movie or episode)" },
      { keys: [["T"]], does: "Theater: the picture fills the window" },
      { keys: [["C"]], does: "Subtitles on or off, kept for what you watch next" },
      { keys: [["R"]], does: "Record the programme on now (not in a movie or episode)" },
      { keys: [["Enter"]], does: "Play the next episode now (Up next)" },
      { keys: [["Esc"]], does: "Cancel Up next" },
      { keys: [["Enter"]], does: "Resume where you stopped (when asked)" },
      { keys: [["Esc"]], does: "Close the Resume question" },
    ],
  },
  {
    title: "In the Movies list",
    when: "on the Movies tab",
    needs: "onDemand",
    items: [
      { keys: [["/"]], does: "Search movies" },
      { keys: [["↑"], ["↓"]], pair: true, does: "Previous or next title" },
      { keys: [["←"], ["→"]], pair: true, does: "Along a row, in the posters" },
      { keys: [["PageUp"], ["PageDown"]], pair: true, does: "A screen up or down" },
      { keys: [["Home"], ["End"]], pair: true, does: "The first or last title" },
      { keys: [["Enter"]], does: "Open the selected title's details" },
    ],
  },
  {
    title: "In the Shows list",
    when: "on the Shows tab",
    needs: "onDemand",
    items: [
      { keys: [["/"]], does: "Search shows" },
      { keys: [["↑"], ["↓"]], pair: true, does: "Previous or next show; its episodes follow" },
      { keys: [["←"], ["→"]], pair: true, does: "Along a row, in the posters" },
      { keys: [["PageUp"], ["PageDown"]], pair: true, does: "A screen up or down" },
      { keys: [["Home"], ["End"]], pair: true, does: "The first or last show" },
      { keys: [["Enter"], ["→"]], does: "Into the selected show's episodes (→ in the list)" },
      { keys: [["Enter"]], does: "In the episodes: play the one chosen" },
      { keys: [["["], ["]"]], pair: true, does: "In the episodes: previous or next season" },
      { keys: [["←"], ["Esc"]], does: "From the episodes back to the list" },
      { keys: [["Esc"]], does: "Close the episodes" },
    ],
  },
  {
    title: "In the guide",
    items: [
      { keys: [["←"], ["→"]], pair: true, does: "An hour earlier or later" },
      { keys: [["Home"]], does: "Back to now" },
      { keys: [["Shift", "Wheel"]], does: "Scroll through time", mouse: true },
    ],
  },
  {
    title: "Mouse",
    items: [
      {
        keys: [["Mouse back"]],
        does: "Back: a menu, a sheet, the player, then the last tab or rail",
        mouse: true,
      },
      {
        keys: [["Mouse forward"]],
        does: "Forward again to the tab or rail choice Back left",
        mouse: true,
      },
      { keys: [["Wheel"]], does: "Over the picture: volume", mouse: true, needs: "playback" },
      {
        keys: [["Click"]],
        does: "On the docked picture: the player, or resume if paused",
        mouse: true,
        needs: "playback",
      },
      {
        keys: [["Double-click"]],
        does: "Middle of the picture: fullscreen",
        mouse: true,
        needs: "playback",
      },
      {
        keys: [["Double-click"]],
        does: "Left or right edge: back or forward 5 seconds",
        mouse: true,
        needs: "playback",
      },
      { keys: [["Right-click"]], does: "A category or favourite: its menu", mouse: true },
      {
        keys: [["Right-click"]],
        does: "On the picture: record, tracks, aspect, sleep timer",
        mouse: true,
        needs: "playback",
      },
    ],
  },
];

/** The groups as they apply here: what needs something that is not here
 *  left out, and a group left with nothing dropped. A browser that cannot
 *  play lists no player keys, and no browser lists mpv's. */
export function shortcutGroups(caps: Capabilities): ShortcutGroup[] {
  const here = (need: Capability | undefined) => need === undefined || caps[need];
  return SHORTCUT_GROUPS.filter((g) => here(g.needs))
    .map((g) => ({ ...g, items: g.items.filter((it) => here(it.needs)) }))
    .filter((g) => g.items.length > 0);
}

/** Keys that are mouse words rather than keyboard keys, for the check. */
export const MOUSE_WORDS = new Set([
  "Wheel",
  "Click",
  "Double-click",
  "Right-click",
  "Mouse back",
  "Mouse forward",
]);
