// The order the category rail draws its rows in: the rail's own items first,
// then pinned categories, then the rest in whichever sort the viewer chose;
// hidden categories left out unless the viewer asked to see them, with a
// closing hint when the filter matches some that are.
// Pure; exercised by src/__checks__/categoryOrder.test.mjs.
//
// Pins are kept in the order they were made, not sorted. A pin is the viewer
// saying "this one matters", and the second thing they pin is usually the
// second thing they reach for — alphabetising would quietly overrule that,
// and would make the pinned block reshuffle itself every time the sort button
// is pressed, when the point of pinning is that those rows stay put. So the
// sort only ever applies below the divider.
//
// Pins are stored by category id, and a pin whose category has gone (the
// provider renamed or dropped it) is kept rather than pruned: categories come
// and go between syncs, a failed or partial sync can briefly return far fewer
// of them, and pruning on that would forget pins the viewer still wants. A
// stale id costs a few bytes and is simply never drawn. Hidden ids are kept
// the same way, for the same reason.
//
// Hiding outranks pinning without undoing it: a hidden category is not drawn
// even if pinned, but its pin stays, so unhiding it puts it back where it was
// rather than dropping it among the rest.
//
// Local rather than in SettingsDto, like the rail's width: which categories
// one person reaches for on this machine is not a property of the account.

import type { FixedRailItem } from "./railItems.ts";

export type CategorySort = "provider" | "az" | "count";

/** The order the sort button steps through. Provider first because it is the
 *  default, and because it is the only order with meaning nobody here can
 *  recompute: the provider's own grouping, which is often by country.
 *  Stored as "provider" whatever it is called on screen. */
export const CATEGORY_SORTS: readonly CategorySort[] = ["provider", "az", "count"];

export const CATEGORY_SORT_DEFAULT: CategorySort = "provider";

export const CATEGORY_PINS_STORAGE_KEY = "zapper.categoryPins";
export const CATEGORY_SORT_STORAGE_KEY = "zapper.categorySort";
export const CATEGORY_HIDDEN_STORAGE_KEY = "zapper.categoryHidden";

/** What each mode is called: `short` on the button, where the rail can be as
 *  narrow as nine rem, `long` in its tooltip, and `menu` in the options menu.
 *  The provider's order is "Default" on the button, a word anyone can read
 *  without knowing how the server is run. */
export const SORT_LABEL: Record<CategorySort, { short: string; long: string; menu: string }> = {
  provider: { short: "Default", long: "Provider's order", menu: "Default (provider's order)" },
  az: { short: "A–Z", long: "A–Z", menu: "A–Z" },
  count: { short: "Most", long: "Most channels", menu: "Most channels" },
};

export function nextSort(sort: CategorySort): CategorySort {
  const i = CATEGORY_SORTS.indexOf(sort);
  return CATEGORY_SORTS[(i + 1) % CATEGORY_SORTS.length] ?? CATEGORY_SORT_DEFAULT;
}

/** The button's tooltip: what it is now, and what a click will make it —
 *  a cycling button that only says the former leaves you clicking to find
 *  out. */
export function sortTip(sort: CategorySort): string {
  return `Sort: ${SORT_LABEL[sort].long} (click for ${SORT_LABEL[nextSort(sort)].long})`;
}

export function parseSort(raw: string | null): CategorySort {
  return (CATEGORY_SORTS as readonly string[]).includes(raw ?? "")
    ? (raw as CategorySort)
    : CATEGORY_SORT_DEFAULT;
}

/** Stored as a JSON array of ids. Anything else — hand-edited, truncated, a
 *  value from some future format — reads as no pins rather than as an error:
 *  losing pins is a nuisance, a rail that will not draw is not an option. */
export function parsePins(raw: string | null): string[] {
  if (raw === null || raw === "") return [];
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(value)) return [];
  const seen = new Set<string>();
  for (const v of value) {
    if (typeof v === "string" && v !== "") seen.add(v);
  }
  return [...seen];
}

export function serialisePins(pins: readonly string[]): string {
  return JSON.stringify(pins);
}

/** Hidden ids are stored exactly as pins are — a JSON array of ids, rubbish
 *  reading as none — so they share the parser. Their order means nothing. */
export const parseHidden = parsePins;
export const serialiseHidden = serialisePins;

/** Pin or unpin one category. Pinning appends, so pins keep the order they
 *  were made in; pinning something already pinned leaves it where it is
 *  rather than moving it to the end. Returns the same array when nothing
 *  changed, so the store can skip the write. */
export function setPinned(pins: readonly string[], id: string, on: boolean): readonly string[] {
  const has = pins.includes(id);
  if (on === has) return pins;
  return on ? [...pins, id] : pins.filter((p) => p !== id);
}

/** Hide or unhide one category; the same set arithmetic as pinning. */
export const setHidden = setPinned;

/** How many of the catalogue's categories are hidden — what "Unhide all"
 *  will bring back. Stale ids are not counted: there is nothing to show for
 *  them, and a count that never reaches zero would be a puzzle. */
export function hiddenCount(categories: readonly { id: string }[], hidden: readonly string[]): number {
  if (hidden.length === 0) return 0;
  const set = new Set(hidden);
  let n = 0;
  for (const c of categories) if (set.has(c.id)) n++;
  return n;
}

/** The fields the ordering reads. CategoryDto has these and more. */
export interface OrderableCategory {
  id: string;
  name: string;
  channelCount: number;
}

export interface CategoryOrderOptions {
  pins: readonly string[];
  sort: CategorySort;
  /** The filter box's text, as typed. */
  filter: string;
  /** Category ids the viewer hid. Absent means none. */
  hidden?: readonly string[];
  /** Draw hidden categories anyway, where they would otherwise be. */
  showHidden?: boolean;
}

/** Built once: a collator is not cheap and this runs over every category
 *  whenever the filter changes. `base` sensitivity so case and accents do not
 *  split "news" from "News" from "Nëws"; `numeric` so "Sports 2" comes before
 *  "Sports 10". Ties fall back to the provider's order, because the sort
 *  below is stable. */
let collator: Intl.Collator | null = null;
function compareNames(a: string, b: string): number {
  collator ??= new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });
  return collator.compare(a, b);
}

/** Split the catalogue into the pinned block and the rest, each in the order
 *  it is drawn.
 *
 *  The filter applies to both halves: a pinned category that matches still
 *  comes first, which is where the viewer will look for it, and one that does
 *  not is hidden like any other — a filter that let pins through regardless
 *  would be a filter that does not filter.
 *
 *  Hidden categories are dropped from both halves before the filter is even
 *  asked; typing a hidden one's name does not bring it back. With
 *  `showHidden` they are drawn exactly where they would be had they never
 *  been hidden — a pinned one in the pinned block — so the viewer can see
 *  what unhiding will restore. */
export function orderCategories<T extends OrderableCategory>(
  categories: readonly T[],
  { pins, sort, filter, hidden = [], showHidden = false }: CategoryOrderOptions,
): { pinned: T[]; rest: T[] } {
  const q = filter.trim().toLowerCase();
  const gone = showHidden || hidden.length === 0 ? null : new Set(hidden);
  const matches = (c: T) =>
    !gone?.has(c.id) && (q === "" || c.name.toLowerCase().includes(q));

  const byId = new Map<string, T>();
  for (const c of categories) byId.set(c.id, c);

  const pinned: T[] = [];
  const pinnedIds = new Set<string>();
  for (const id of pins) {
    const c = byId.get(id);
    // Unknown ids are kept in storage and skipped here; see the note at the
    // top. A duplicate id cannot draw the same row twice either.
    if (!c || pinnedIds.has(id)) continue;
    pinnedIds.add(id);
    if (matches(c)) pinned.push(c);
  }

  const rest = categories.filter((c) => !pinnedIds.has(c.id) && matches(c));
  if (sort === "az") {
    rest.sort((a, b) => compareNames(a.name, b.name));
  } else if (sort === "count") {
    rest.sort((a, b) => b.channelCount - a.channelCount);
  }
  return { pinned, rest };
}

export const HINT_ROW_ID = "*hidden-hint*";

/** One row of the rail as the component draws it. */
export interface RailRow {
  id: string;
  name: string;
  /** Text, already formatted; blank when there is nothing to count. */
  count: string;
  kind: "fixed" | "category" | "hint";
  pinned: boolean;
  /** Hidden, and drawn only because hidden categories are being shown. */
  hidden: boolean;
  /** The last of the fixed and pinned block, which carries the divider under
   *  it. A property of the row rather than a row of its own, because the rail
   *  is windowed on a fixed row height and a divider row would be a 26-pixel
   *  gap — or a second height for the window arithmetic to know about. */
  pinEnd: boolean;
  /** Where the filter matched in the name, as [start, end), else null. */
  match: readonly [number, number] | null;
}

/** Where the filter first occurs in a name, trimmed and ignoring case.
 *  Offsets are into `name`; lower-casing can change a string's length for a
 *  few scripts, and then no highlight is reported rather than a wrong one. */
export function matchRange(name: string, filter: string): readonly [number, number] | null {
  const q = filter.trim().toLowerCase();
  if (q === "") return null;
  const lower = name.toLowerCase();
  if (lower.length !== name.length) return null;
  const at = lower.indexOf(q);
  return at < 0 ? null : [at, at + q.length];
}

/** How many hidden categories the filter matches: the ones the rail is not
 *  drawing but would be, had they not been hidden. Zero without a filter. */
export function hiddenMatches(
  categories: readonly OrderableCategory[],
  hidden: readonly string[],
  filter: string,
): number {
  const q = filter.trim().toLowerCase();
  if (q === "" || hidden.length === 0) return 0;
  const set = new Set(hidden);
  let n = 0;
  for (const c of categories) if (set.has(c.id) && c.name.toLowerCase().includes(q)) n++;
  return n;
}

export function hiddenHint(n: number): string {
  return n === 1 ? "1 more in a hidden category" : `${n} more in hidden categories`;
}

/** Every row of the rail: the fixed items first, whatever is pinned, sorted,
 *  filtered or hidden (they are not categories, they are the ways into the
 *  views), then the pinned categories, then the rest, then a hint row when the
 *  filter matches hidden categories that are not being shown. */
export function railRows(
  fixed: readonly FixedRailItem[],
  categories: readonly OrderableCategory[],
  options: CategoryOrderOptions,
): RailRow[] {
  const { pinned, rest } = orderCategories(categories, options);
  const hiddenIds = options.hidden ?? [];
  const hiddenSet = new Set(hiddenIds);
  const row = (c: OrderableCategory, isPinned: boolean): RailRow => ({
    id: c.id,
    name: c.name,
    count: c.channelCount ? c.channelCount.toLocaleString() : "",
    kind: "category",
    pinned: isPinned,
    hidden: hiddenSet.has(c.id),
    pinEnd: false,
    match: matchRange(c.name, options.filter),
  });
  const rows: RailRow[] = [
    ...fixed.map(
      (f): RailRow => ({ ...f, kind: "fixed", pinned: false, hidden: false, pinEnd: false, match: null }),
    ),
    ...pinned.map((c) => row(c, true)),
    ...rest.map((c) => row(c, false)),
  ];
  const stray = options.showHidden ? 0 : hiddenMatches(categories, hiddenIds, options.filter);
  if (stray > 0) {
    rows.push({
      id: HINT_ROW_ID,
      name: hiddenHint(stray),
      count: "",
      kind: "hint",
      pinned: false,
      hidden: false,
      pinEnd: false,
      match: null,
    });
  }
  // No divider under the block when there is nothing below it to divide
  // from: a filter that only matches pins would otherwise end on a rule.
  const blockEnd = fixed.length + pinned.length - 1;
  if (blockEnd >= 0 && rows.length > blockEnd + 1) rows[blockEnd]!.pinEnd = true;
  return rows;
}
