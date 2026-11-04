// The category rail's order: pins, then the sort, under the filter.
//
// The rail is windowed, so a wrong order is not a cosmetic problem — the row
// offsets, the divider and the "All channels" escape hatch all come from the
// list this produces.
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fixedRailItems } from "../logic/railItems.ts";
import {
  HINT_ROW_ID,
  hiddenHint,
  hiddenMatches,
  matchRange,
  CATEGORY_SORTS,
  CATEGORY_SORT_DEFAULT,
  SORT_LABEL,
  hiddenCount,
  nextSort,
  orderCategories,
  parseHidden,
  parsePins,
  parseSort,
  railRows,
  serialiseHidden,
  serialisePins,
  setHidden,
  setPinned,
  sortTip,
} from "../logic/categoryOrder.ts";

// The provider's own order, deliberately neither alphabetical nor by size.
const CATS = [
  { id: "uk", name: "UK | News", channelCount: 40 },
  { id: "s10", name: "Sports 10", channelCount: 5 },
  { id: "fr", name: "france", channelCount: 120 },
  { id: "s2", name: "Sports 2", channelCount: 5 },
  { id: "doc", name: "Documentaries", channelCount: 60 },
];

const FIXED = fixedRailItems({ favourites: 18, recent: 10, recordings: "", total: 230 });
const F = FIXED.map((f) => f.id);
const ids = (xs) => xs.map((x) => x.id);
const opts = (o) => ({ pins: [], sort: "provider", filter: "", ...o });

test("pins come first, in the order they were pinned", () => {
  const { pinned, rest } = orderCategories(CATS, opts({ pins: ["doc", "uk"] }));
  assert.deepEqual(ids(pinned), ["doc", "uk"]);
  assert.deepEqual(ids(rest), ["s10", "fr", "s2"]);
});

test("a pinned category is not drawn twice, even if stored twice", () => {
  const rows = railRows(FIXED, CATS, opts({ pins: ["fr", "fr", "s2"] }));
  const seen = ids(rows);
  assert.equal(new Set(seen).size, seen.length);
  assert.deepEqual(seen, [...F, "fr", "s2", "uk", "s10", "doc"]);
});

test("a pin for a category that no longer exists is skipped, not an error", () => {
  const { pinned, rest } = orderCategories(CATS, opts({ pins: ["gone", "s2"] }));
  assert.deepEqual(ids(pinned), ["s2"]);
  assert.equal(rest.length, CATS.length - 1);
});

test("provider order is the catalogue's own", () => {
  assert.deepEqual(ids(orderCategories(CATS, opts()).rest), ids(CATS));
});

test("A–Z ignores case and counts numbers as numbers", () => {
  const { rest } = orderCategories(CATS, opts({ sort: "az" }));
  // "france" is lower case and still lands between D and S; "Sports 2"
  // before "Sports 10".
  assert.deepEqual(ids(rest), ["doc", "fr", "s2", "s10", "uk"]);
});

test("most channels is largest first, ties kept in provider order", () => {
  const { rest } = orderCategories(CATS, opts({ sort: "count" }));
  assert.deepEqual(ids(rest), ["fr", "doc", "uk", "s10", "s2"]);
});

test("the sort applies below the pins, never to them", () => {
  for (const sort of CATEGORY_SORTS) {
    const { pinned } = orderCategories(CATS, opts({ sort, pins: ["uk", "s10", "fr"] }));
    assert.deepEqual(ids(pinned), ["uk", "s10", "fr"], sort);
  }
});

test("the filter hides non-matching pins and floats matching ones", () => {
  const { pinned, rest } = orderCategories(
    CATS,
    opts({ pins: ["uk", "s10"], filter: "  SPORTS " }),
  );
  assert.deepEqual(ids(pinned), ["s10"]);
  assert.deepEqual(ids(rest), ["s2"]);
});


test("pinning appends, unpinning removes, and a no-op is the same array", () => {
  const a = setPinned([], "x", true);
  const b = setPinned(a, "y", true);
  assert.deepEqual(b, ["x", "y"]);
  assert.equal(setPinned(b, "x", true), b, "re-pinning does not move it to the end");
  assert.equal(setPinned(b, "z", false), b);
  assert.deepEqual(setPinned(b, "x", false), ["y"]);
});

test("stored pins survive a round trip, and rubbish reads as none", () => {
  assert.deepEqual(parsePins(serialisePins(["b", "a"])), ["b", "a"]);
  assert.deepEqual(parsePins(null), []);
  assert.deepEqual(parsePins(""), []);
  assert.deepEqual(parsePins("{not json"), []);
  assert.deepEqual(parsePins('{"a":1}'), []);
  assert.deepEqual(parsePins('["a", 3, null, "", "a", "b"]'), ["a", "b"]);
});

test("the sort cycles provider → A–Z → most → provider, and parses safely", () => {
  assert.equal(CATEGORY_SORT_DEFAULT, "provider");
  assert.equal(nextSort("provider"), "az");
  assert.equal(nextSort("az"), "count");
  assert.equal(nextSort("count"), "provider");
  for (const s of CATEGORY_SORTS) assert.equal(parseSort(s), s);
  assert.equal(parseSort(null), "provider");
  assert.equal(parseSort("alphabetical"), "provider");
  assert.equal(sortTip("az"), "Sort: A–Z (click for Most channels)");
  assert.equal(sortTip("count"), "Sort: Most channels (click for Provider's order)");
});

// "Panel" was the provider's order's name on the button, and nobody knew
// where it came from. The stored value stays "provider" so saved prefs read.
test("the provider's order is called Default, never Panel", () => {
  assert.equal(SORT_LABEL.provider.short, "Default");
  assert.equal(SORT_LABEL.provider.long, "Provider's order");
  assert.equal(SORT_LABEL.provider.menu, "Default (provider's order)");
  assert.equal(sortTip("provider"), "Sort: Provider's order (click for A–Z)");
  for (const s of CATEGORY_SORTS) {
    for (const label of Object.values(SORT_LABEL[s])) assert.doesNotMatch(label, /panel/i);
  }
  assert.equal(parseSort("provider"), "provider");
});

test("hidden categories are left out, pinned ones too", () => {
  const { pinned, rest } = orderCategories(
    CATS,
    opts({ pins: ["doc", "uk"], hidden: ["uk", "s10"] }),
  );
  assert.deepEqual(ids(pinned), ["doc"]);
  // A hidden pin is not demoted into the rest either.
  assert.deepEqual(ids(rest), ["fr", "s2"]);
});

test("the filter does not bring a hidden category back", () => {
  const { pinned, rest } = orderCategories(CATS, opts({ hidden: ["s2"], filter: "sports" }));
  assert.deepEqual(ids(pinned), []);
  assert.deepEqual(ids(rest), ["s10"]);
});

test("shown hidden categories sit where they would be, marked", () => {
  for (const sort of CATEGORY_SORTS) {
    const plain = railRows(FIXED, CATS, opts({ sort, pins: ["uk"] }));
    const shown = railRows(
      FIXED,
      CATS,
      opts({ sort, pins: ["uk"], hidden: ["uk", "fr"], showHidden: true }),
    );
    assert.deepEqual(ids(shown), ids(plain), sort);
    assert.deepEqual(
      shown.filter((r) => r.hidden).map((r) => r.id).sort(),
      ["fr", "uk"],
      sort,
    );
    // Still pinned while hidden, so it is in the pinned block when shown.
    assert.equal(shown.find((r) => r.id === "uk").pinned, true);
  }
  // And the filter still applies to them.
  const filtered = railRows(FIXED, CATS, opts({ hidden: ["s2"], showHidden: true, filter: "sports" }));
  assert.deepEqual(ids(filtered), [...F, "s10", "s2"]);
});

test("hiding the only rows below the pins takes the divider with them", () => {
  const rows = railRows(FIXED, CATS, opts({ pins: ["doc"], hidden: ["uk", "s10", "fr", "s2"] }));
  assert.deepEqual(ids(rows), [...F, "doc"]);
  assert.equal(rows.some((r) => r.pinEnd), false);
  // Hiding every pin leaves no pinned block; the divider falls under the fixed items.
  const none = railRows(FIXED, CATS, opts({ pins: ["doc"], hidden: ["doc"] }));
  assert.equal(none.some((r) => r.pinned), false);
  assert.deepEqual(none.filter((r) => r.pinEnd).map((r) => r.id), ["*all*"]);
  assert.equal(none.length, F.length + CATS.length - 1);
});

test("All channels cannot be hidden by a stored id", () => {
  const rows = railRows(FIXED, CATS, opts({ hidden: ["*all*"] }));
  assert.equal(rows.find((r) => r.id === "*all*").hidden, false);
});

test("the fixed items lead, in order, whatever is pinned, sorted, filtered or hidden", () => {
  for (const sort of CATEGORY_SORTS) for (const filter of ["", "sports", "no such thing"]) {
    const rows = railRows(FIXED, CATS, opts({ sort, filter, pins: ["doc"], hidden: [...F] }));
    assert.deepEqual(ids(rows.slice(0, F.length)), F);
    assert.ok(rows.slice(0, F.length).every((r) => r.kind === "fixed" && !r.pinned && !r.hidden && r.match === null));
  }
});
test("the divider closes the fixed and pinned block, only when rows follow it", () => {
  const rows = railRows(FIXED, CATS, opts({ pins: ["doc", "uk"] }));
  assert.deepEqual(rows.filter((r) => r.pinEnd).map((r) => r.id), ["uk"]);
  assert.deepEqual(railRows(FIXED, CATS, opts()).filter((r) => r.pinEnd).map((r) => r.id), ["*all*"]);
  assert.equal(railRows(FIXED, CATS, opts({ pins: ["doc"], filter: "docu" })).some((r) => r.pinEnd), false);
  assert.equal(railRows(FIXED, CATS, opts({ filter: "no such thing" })).some((r) => r.pinEnd), false);
});
test("a filter marks where each category matched, ignoring case", () => {
  assert.deepEqual(matchRange("UK | SPORTS ▸ FOOTBALL", "  sports "), [5, 11]);
  assert.equal(matchRange("UK | News", ""), null);
  assert.equal(matchRange("UK | News", "xyz"), null);
  const rows = railRows(FIXED, CATS, opts({ filter: "sports" }));
  assert.deepEqual(rows.find((r) => r.id === "s2").match, [0, 6]);
});
test("matches among hidden categories are counted under the list, not shown", () => {
  const rows = railRows(FIXED, CATS, opts({ hidden: ["s2"], filter: "sports" }));
  const last = rows[rows.length - 1];
  assert.deepEqual([last.id, last.kind, last.name], [HINT_ROW_ID, "hint", "1 more in a hidden category"]);
  assert.equal(hiddenHint(2), "2 more in hidden categories");
  assert.equal(railRows(FIXED, CATS, opts({ hidden: ["s2"], filter: "sports", showHidden: true })).some((r) => r.kind === "hint"), false);
  assert.equal(railRows(FIXED, CATS, opts({ hidden: ["s2"] })).some((r) => r.kind === "hint"), false);
  assert.equal(hiddenMatches(CATS, ["s2", "gone"], "sports"), 1);
});
test("counts are words: a category with no channels says nothing", () => {
  const rows = railRows(FIXED, [{ id: "z", name: "Zero", channelCount: 0 }, ...CATS], opts());
  assert.equal(rows.find((r) => r.id === "z").count, "");
  assert.equal(rows.find((r) => r.id === "fr").count, (120).toLocaleString());
});

test("hiding and unhiding are set operations; stale ids are kept, not counted", () => {
  const a = setHidden([], "fr", true);
  assert.deepEqual(a, ["fr"]);
  assert.equal(setHidden(a, "fr", true), a);
  assert.deepEqual(setHidden(a, "fr", false), []);
  assert.equal(hiddenCount(CATS, ["fr", "gone", "uk"]), 2);
  assert.equal(hiddenCount(CATS, []), 0);
  // An unknown hidden id neither errors nor hides anything.
  const { rest } = orderCategories(CATS, opts({ hidden: ["gone"] }));
  assert.deepEqual(ids(rest), ids(CATS));
});

test("stored hidden ids survive a round trip, and rubbish reads as none", () => {
  assert.deepEqual(parseHidden(serialiseHidden(["gone", "fr"])), ["gone", "fr"]);
  assert.deepEqual(parseHidden(null), []);
  assert.deepEqual(parseHidden("[1,2"), []);
  assert.deepEqual(parseHidden('"fr"'), []);
  assert.deepEqual(parseHidden('["fr", false, "fr", ""]'), ["fr"]);
});

// The sort button sits in the rail's filter row, and that row has to stay
// exactly as tall as the channel list's search row beside it. The button gets
// there by stretching to the filter box's height rather than having one of
// its own: give it a height or vertical padding and a zoom level or a font
// change can make it the taller of the two, pushing the rail's rows down by
// a pixel against the list's. Measured in the browser too; this is what
// keeps it from regressing quietly.
//
// The ▾ beside it, and the box holding the two, follow the same rule.
test("the sort buttons take the filter box's height instead of setting one", () => {
  const css = readFileSync(new URL("../styles.css", import.meta.url), "utf8").replace(
    /\/\*[\s\S]*?\*\//g,
    "",
  );
  const block = (sel) => {
    const m = new RegExp(`^\\.${sel}\\s*\\{([^}]*)\\}`, "m").exec(css);
    assert.ok(m, `.${sel} is missing from the stylesheet`);
    return m[1];
  };
  const noHeight = (sel, body) =>
    assert.doesNotMatch(
      body,
      /(^|[\s;])(height|min-height|max-height|padding-top|padding-bottom|block-size):/,
      `.${sel} sets a height of its own`,
    );
  for (const sel of ["rail-sort", "rail-sort-more"]) {
    const body = block(sel);
    assert.match(body, /align-self:\s*stretch/, `.${sel} stretches`);
    noHeight(sel, body);
    const padding = /(^|[\s;])padding:\s*([^;]+);/.exec(body);
    assert.ok(padding, `.${sel} sets its padding explicitly`);
    assert.match(padding[2].trim(), /^0(\s|$)/, `no vertical padding on .${sel}`);
  }
  // The pair's box: stretched to the row and adding nothing to it.
  const split = block("rail-sort-split");
  assert.match(split, /align-self:\s*stretch/);
  noHeight("rail-sort-split", split);
  assert.doesNotMatch(split, /(^|[\s;])(padding|border|margin)(-top|-bottom)?:/);
});
