// Every keyboard shortcut and mouse gesture, as data the settings pane draws.
//
// Kept as data, beside the handlers rather than inside the pane's markup,
// because a list written as prose drifts from the keys it describes.
// src/__checks__/shortcuts.test.mjs checks the keys listed here
// against the handler's source, so a key that stops working leaves the list.
//
// Pure; no React.

/** One way to press it: the keys of a chord, held together. */
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
}

export interface ShortcutGroup {
  title: string;
  /** When the group applies, said once instead of on every row. */
  when?: string;
  items: readonly Shortcut[];
}

export const SHORTCUT_GROUPS: readonly ShortcutGroup[] = [
  {
    title: "Anywhere",
    items: [
      { keys: [["Ctrl", "P"]], does: "Command palette — every action, and channel search" },
      { keys: [["/"]], does: "Search channels" },
      { keys: [["G"]], does: "Switch between the list and the guide" },
      { keys: [["S"]], does: "Stats for nerds" },
      { keys: [["P"]], does: "Previous channel; again to come back" },
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
    items: [
      { keys: [["Space"]], does: "Play or pause" },
      { keys: [["←"], ["→"]], pair: true, does: "Back or forward 5 seconds" },
      { keys: [["L"]], does: "Jump to live" },
      { keys: [["T"]], does: "Theater: the picture fills the window" },
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
      { keys: [["Wheel"]], does: "Over the picture: volume", mouse: true },
      { keys: [["Click"]], does: "On the docked picture: the player, or resume if paused", mouse: true },
      { keys: [["Double-click"]], does: "Middle of the picture: fullscreen", mouse: true },
      { keys: [["Double-click"]], does: "Left or right edge: back or forward 5 seconds", mouse: true },
      { keys: [["Right-click"]], does: "A category or favourite: its menu", mouse: true },
    ],
  },
];

/** Keys that are mouse words rather than keyboard keys, for the check. */
export const MOUSE_WORDS = new Set(["Wheel", "Click", "Double-click", "Right-click"]);
