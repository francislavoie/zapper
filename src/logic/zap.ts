// Tuning by channel number, the way a TV remote does it: type the digits and
// the set changes channel once it knows what you meant.
//
// "Once it knows" is the whole design. Three things end the entry:
//
//  - the number cannot go on: no channel number starts with what has been
//    typed and is longer. A catalogue numbered 1–20 tunes "3" at once, and
//    one whose longest number has four digits tunes on the fourth;
//  - Enter, for "that one, now" while a longer number is still possible;
//  - a pause (ZAP_IDLE_MS) with no key, the remote's own rule.
//
// What is typed is compared with the number as the list writes it — the
// label, so an over-the-air "5.1" is typed with its dot — and leading zeros
// are ignored, so "007" is channel 7, as on a remote with a fixed width.
//
// The index is built from the backend's `channel_numbers`: the whole
// catalogue, not the category on screen, because a number names a channel
// wherever it is filed.
//
// Pure; no React. Checked in src/__checks__/zap.test.mjs.

/** How long a pause ends the entry. Long enough to find the next key on a
 *  number row without looking, short enough not to feel like waiting. */
export const ZAP_IDLE_MS = 1800;

/** How long "No channel 47" stays up before the entry goes away. */
export const ZAP_MISS_MS = 1500;

/** How long the chosen number and name stay up after it tunes, so the eye
 *  that was on the keys can catch what it landed on. */
export const ZAP_TUNED_MS = 1200;

/** Longer than any real channel number. Keeps a held key from typing a line
 *  of digits when the catalogue is not loaded yet to say "no such number". */
export const ZAP_MAX_TYPED = 8;

export interface NumberIndex {
  /** Every distinct label, sorted as strings, for prefix searches. */
  labels: readonly string[];
  /** The channel each label tunes: the first in list order. */
  first: ReadonlyMap<string, number>;
  /** The longest label, in characters. */
  longest: number;
}

export function buildNumberIndex(rows: readonly (readonly [string, number])[]): NumberIndex {
  const first = new Map<string, number>();
  let longest = 0;
  for (const [raw, streamId] of rows) {
    const label = raw.trim();
    if (label === "" || first.has(label)) continue;
    first.set(label, streamId);
    longest = Math.max(longest, label.length);
  }
  const labels = [...first.keys()].sort();
  return { labels, first, longest };
}

/** What a key types into the entry, or null when it is not part of a
 *  number. Digits from the number row or the keypad, and a decimal point for
 *  subchannels — the keypad's, whatever the locale prints on it.
 *
 *  Ctrl, Alt and Meta make it a different shortcut (Ctrl+0 resets the zoom).
 *  Shift does not: on an AZERTY keyboard the number row needs it. */
export function zapChar(e: {
  key: string;
  code?: string;
  ctrlKey?: boolean;
  altKey?: boolean;
  metaKey?: boolean;
}): string | null {
  if (e.ctrlKey || e.altKey || e.metaKey) return null;
  if (e.code === "NumpadDecimal") return ".";
  if (e.key.length === 1 && e.key >= "0" && e.key <= "9") return e.key;
  if (e.key === ".") return ".";
  return null;
}

/** The entry after one more character. A point only after a digit, and only
 *  one; past ZAP_MAX_TYPED nothing more is taken. */
export function typeChar(typed: string, ch: string): string {
  if (typed.length >= ZAP_MAX_TYPED) return typed;
  if (ch === ".") {
    if (typed === "" || typed.includes(".")) return typed;
    return typed + ch;
  }
  return /^[0-9]$/.test(ch) ? typed + ch : typed;
}

/** What has been typed, as the label it would match: leading zeros dropped,
 *  but never the last digit before the point or the end ("0", "0.1"). */
export function numberKey(typed: string): string {
  return typed.replace(/^0+(?=\d)/, "");
}

/** First index in sorted `labels` not less than `prefix`. */
function lowerBound(labels: readonly string[], prefix: string): number {
  let lo = 0;
  let hi = labels.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (labels[mid] < prefix) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

export interface Resolution {
  /** The channel the entry names exactly, as typed so far. */
  exact: number | null;
  /** The label it matched, for saying which number was meant. */
  label: string;
  /** Whether a longer number still starts with what has been typed. */
  more: boolean;
  /** How many characters the entry could run to: what is typed plus room
   *  for the longest number still possible. Drawn as `1 2 _`. */
  slots: number;
}

export function resolve(index: NumberIndex, typed: string): Resolution {
  const lead = typed.length - numberKey(typed).length;
  const key = numberKey(typed);

  // Nothing but zeros: a remote's "0 0 _" on its way to a number, unless the
  // catalogue really has a channel 0 and nothing else could follow.
  if (/^0+$/.test(typed)) {
    const more = typed.length < index.longest;
    return {
      exact: index.first.get("0") ?? null,
      label: "0",
      more,
      slots: Math.max(typed.length, index.longest),
    };
  }

  let more = false;
  let longest = 0;
  for (let i = lowerBound(index.labels, key); i < index.labels.length; i++) {
    const l = index.labels[i];
    if (!l.startsWith(key)) break;
    if (l.length > key.length) more = true;
    longest = Math.max(longest, l.length);
  }
  // Leading zeros count against the width, as on a remote with a fixed
  // number of digits: "00005" in a catalogue whose longest number is five
  // long is finished, whatever else starts with 5.
  const full = typed.length >= index.longest;
  return {
    exact: index.first.get(key) ?? null,
    label: key,
    more: more && !full,
    slots: full ? typed.length : Math.min(index.longest, Math.max(typed.length, lead + longest)),
  };
}

export type ZapDecision =
  /** Keep listening. */
  | { kind: "wait" }
  /** Tune this channel now. */
  | { kind: "tune"; streamId: number; label: string }
  /** No channel has this number, nor any longer number starting with it. */
  | { kind: "miss"; label: string };

/** What a keystroke leads to: tune as soon as nothing longer is possible,
 *  give up as soon as nothing at all is. */
export function afterKey(index: NumberIndex, typed: string): ZapDecision {
  const r = resolve(index, typed);
  if (r.more) return { kind: "wait" };
  return r.exact !== null
    ? { kind: "tune", streamId: r.exact, label: r.label }
    : { kind: "miss", label: r.label };
}

/** What Enter or the pause leads to: whatever the entry names now. */
export function settle(index: NumberIndex, typed: string): ZapDecision {
  const r = resolve(index, typed);
  return r.exact !== null
    ? { kind: "tune", streamId: r.exact, label: r.label }
    : { kind: "miss", label: r.label };
}

/** The entry as drawn: each character, then a blank per slot still open.
 *  `["1", "2", "_"]`. */
export function zapCells(typed: string, slots: number): string[] {
  const cells = [...typed];
  for (let i = typed.length; i < slots; i++) cells.push("_");
  return cells;
}

/** The entry as the overlay draws it. Held in the store because the picture's
 *  corner and the floating chip both draw it, and whichever is on screen has
 *  to show the same thing. */
export interface ZapView {
  /** "typing" while keys are coming; "tuned" for a moment after a channel
   *  was chosen; "miss" for a moment after a number nothing has. */
  status: "typing" | "tuned" | "miss";
  /** What was typed, and the blanks still open: ["1", "2", "_"]. */
  cells: string[];
  /** The number as matched, for "No channel 47". */
  label: string;
  /** The channel the entry names as it stands, when it names one. */
  name: string | null;
}

/** The first point at which an entry was finished, reading it a key at a
 *  time: what that part of it decided, and the keys typed after it, which
 *  begin a new entry.
 *
 *  Keys normally arrive one by one and each is decided as it comes, so the
 *  answer is the whole entry. This is for keys typed before the catalogue's
 *  numbers had arrived: "70001" typed quickly into a catalogue whose numbers
 *  stop at 60000 is channel 7000 and then the start of a 1, exactly as it
 *  would have been had the numbers been there all along. */
export function firstDecision(
  index: NumberIndex,
  typed: string,
): { decision: ZapDecision; rest: string } {
  for (let i = 1; i < typed.length; i++) {
    const d = afterKey(index, typed.slice(0, i));
    // A point cannot start the next entry; it is dropped with the break.
    if (d.kind !== "wait") return { decision: d, rest: typed.slice(i).replace(/^\./, "") };
  }
  return { decision: afterKey(index, typed), rest: "" };
}
