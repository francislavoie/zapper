// Which modifier is the shortcut key, and how a shortcut is written, on the
// platform the app is running on.
//
// The shortcut data (logic/shortcuts.ts, the palette's hints) is written once,
// in PC terms: "Ctrl" there means "the shortcut modifier", the key a desktop
// app's shortcuts hang off. On Linux and Windows that is Control. On a Mac it
// is Command, Control is a different key that means something else (and
// Control with a letter is often the system's), and shortcuts are written
// with Apple's symbols in Apple's order: ⌃⌥⇧⌘, then the key, with nothing
// between them.
//
// Pure; no DOM. The one place that asks what the machine is is src/platform.ts.

/** What the platform is, as far as shortcuts care. */
export interface PlatformHints {
  /** `navigator.userAgentData.platform`, where the engine has it. */
  uaPlatform?: string;
  /** `navigator.platform`: "MacIntel" on every Mac, Apple Silicon included. */
  platform?: string;
  userAgent?: string;
  /** The page's query string; `?platform=mac` forces the Mac look, for the
   *  browser mock on another machine. */
  search?: string;
}

/** Whether these describe a Mac. */
export function isMacPlatform(h: PlatformHints): boolean {
  const forced = new URLSearchParams(h.search ?? "").get("platform");
  if (forced === "mac") return true;
  if (forced) return false;
  const p = (h.uaPlatform || h.platform || "").toLowerCase();
  if (p) return p.startsWith("mac");
  return /Macintosh|Mac OS X/.test(h.userAgent ?? "");
}

/** Only the modifiers of a keyboard event, so this can be checked without a
 *  DOM. */
export interface ModifierState {
  ctrlKey?: boolean;
  metaKey?: boolean;
}

/** Whether the shortcut modifier is down.
 *
 *  Elsewhere Ctrl, or the Windows/Super key as a fallback (as it always was
 *  here). On a Mac, Command and only Command: Control+P there is the text
 *  field's "previous line", not the palette, and Control+Command is the
 *  system's own family of shortcuts. */
export function shortcutModifier(e: ModifierState, mac: boolean): boolean {
  if (mac) return Boolean(e.metaKey) && !e.ctrlKey;
  return Boolean(e.ctrlKey || e.metaKey);
}

/** On a Mac, whether a modifier is held that makes a plain-letter shortcut
 *  someone else's: ⌘F is not F, and ⌃F is not either. Never true elsewhere,
 *  where the letters have always fired whatever was held. */
export function blocksPlainKey(e: ModifierState, mac: boolean): boolean {
  return mac && Boolean(e.metaKey || e.ctrlKey);
}

/** The modifiers as the data spells them, in the order a Mac writes them. */
const MAC_ORDER = ["Control", "Alt", "Shift", "Ctrl"] as const;
const MAC_SYMBOL: Record<string, string> = {
  // A real Control key, for the rare shortcut that wants Control on a Mac too.
  Control: "⌃",
  Alt: "⌥",
  Shift: "⇧",
  // The shortcut modifier.
  Ctrl: "⌘",
};

/** The keys of a chord as they should be drawn, one keycap each.
 *
 *  Elsewhere unchanged ("Control" written as "Ctrl", the same key). On a Mac
 *  the modifiers become ⌃⌥⇧⌘ in that order, ahead of the rest, which keep
 *  theirs. */
export function chordKeys(chord: readonly string[], mac: boolean): string[] {
  if (!mac) return chord.map((k) => (k === "Control" ? "Ctrl" : k));
  const mods = MAC_ORDER.filter((m) => chord.includes(m)).map((m) => MAC_SYMBOL[m]);
  const rest = chord.filter((k) => !(k in MAC_SYMBOL));
  return [...mods, ...rest];
}

/** A chord as one piece of text: "Ctrl+P" elsewhere, "⌘P" on a Mac. */
export function chordLabel(chord: readonly string[], mac: boolean): string {
  const keys = chordKeys(chord, mac);
  if (!mac) return keys.join("+");
  // Modifier symbols run together; a word key after them gets a thin space so
  // "⌘Space" does not read as one word.
  const mods = keys.filter((k) => Object.values(MAC_SYMBOL).includes(k)).join("");
  const rest = keys.filter((k) => !Object.values(MAC_SYMBOL).includes(k)).join(" ");
  return rest.length > 1 && mods ? `${mods} ${rest}` : mods + rest;
}

/** A hint written in PC terms ("Ctrl+Q"), for this platform. Anything that is
 *  not a modifier chord ("Esc", "P · last · recall", "⚙") is left as it is. */
export function hintLabel(hint: string, mac: boolean): string {
  const parts = hint.split("+");
  if (parts.length < 2 || !parts.slice(0, -1).every((p) => p in MAC_SYMBOL)) return hint;
  return chordLabel(parts, mac);
}
