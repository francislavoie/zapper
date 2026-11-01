// The order the category rail draws its rows in: pinned categories first, the
// rest in whichever sort the viewer chose, and "All channels" above both.
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
// stale id costs a few bytes and is simply never drawn.
//
// Local rather than in SettingsDto, like the rail's width: which categories
// one person reaches for on this machine is not a property of the account.

export type CategorySort = "provider" | "az" | "count";

/** The order the sort button steps through. Provider first because it is the
 *  default, and because it is the only order with meaning nobody here can
 *  recompute: the panel's own grouping, which is often by country. */
export const CATEGORY_SORTS: readonly CategorySort[] = ["provider", "az", "count"];

export const CATEGORY_SORT_DEFAULT: CategorySort = "provider";

export const CATEGORY_PINS_STORAGE_KEY = "zapper.categoryPins";
export const CATEGORY_SORT_STORAGE_KEY = "zapper.categorySort";

/** What each mode is called: `short` on the button, where the rail can be as
 *  narrow as nine rem, and `long` in its tooltip. */
export const SORT_LABEL: Record<CategorySort, { short: string; long: string }> = {
  provider: { short: "Panel", long: "Provider order" },
  az: { short: "A–Z", long: "A–Z" },
  count: { short: "Most", long: "Most channels" },
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

/** Pin or unpin one category. Pinning appends, so pins keep the order they
 *  were made in; pinning something already pinned leaves it where it is
 *  rather than moving it to the end. Returns the same array when nothing
 *  changed, so the store can skip the write. */
export function setPinned(pins: readonly string[], id: string, on: boolean): readonly string[] {
  const has = pins.includes(id);
  if (on === has) return pins;
  return on ? [...pins, id] : pins.filter((p) => p !== id);
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
 *  would be a filter that does not filter. */
export function orderCategories<T extends OrderableCategory>(
  categories: readonly T[],
  { pins, sort, filter }: CategoryOrderOptions,
): { pinned: T[]; rest: T[] } {
  const q = filter.trim().toLowerCase();
  const matches = (c: T) => q === "" || c.name.toLowerCase().includes(q);

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

/** One row of the rail as the component draws it. */
export interface RailRow {
  id: string;
  name: string;
  count: number;
  pinned: boolean;
  /** The last of the pinned block, which carries the divider under it. A
   *  property of the row rather than a row of its own, because the rail is
   *  windowed on a fixed row height and a divider row would be a 26-pixel
   *  gap — or a second height for the window arithmetic to know about. */
  pinEnd: boolean;
}

/** Every row of the rail, "All channels" first. It stays row zero whatever is
 *  pinned or however the rest is sorted: it is not a category, it is the way
 *  out of one. */
export function railRows(
  all: { id: string; name: string; count: number },
  categories: readonly OrderableCategory[],
  options: CategoryOrderOptions,
): RailRow[] {
  const { pinned, rest } = orderCategories(categories, options);
  const row = (c: OrderableCategory, isPinned: boolean, pinEnd: boolean): RailRow => ({
    id: c.id,
    name: c.name,
    count: c.channelCount,
    pinned: isPinned,
    pinEnd,
  });
  return [
    { ...all, pinned: false, pinEnd: false },
    // No divider under the block when there is nothing below it to divide
    // from: a filter that only matches pins would otherwise end on a rule.
    ...pinned.map((c, i) => row(c, true, i === pinned.length - 1 && rest.length > 0)),
    ...rest.map((c) => row(c, false, false)),
  ];
}
