// Alternatives in a search: `news|weather` finds either, and each side's words
// must all match. The backend has its own half (catalog/search.rs and
// guide_search.rs, with Rust tests); this pins the frontend's, which the
// palette's action filter uses.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import { alternatives, anyAlternative } from "../logic/searchQuery.ts";
import { actionMatches } from "../logic/palette.ts";

test("pipes split a query into trimmed alternatives", () => {
  assert.deepEqual(alternatives("news|weather"), ["news", "weather"]);
  assert.deepEqual(alternatives(" news | weather uk |"), ["news", "weather uk"]);
  assert.deepEqual(alternatives("plain words"), ["plain words"]);
  assert.deepEqual(alternatives("||"), []);
  assert.deepEqual(alternatives(""), []);
});

test("any alternative is enough, and none at all matches everything", () => {
  const has = (word) => (alt) => alt === word;
  assert.equal(anyAlternative("a|b", has("b")), true);
  assert.equal(anyAlternative("a|b", has("c")), false);
  assert.equal(anyAlternative(" | ", has("c")), true);
});

test("the palette's actions take alternatives too", () => {
  assert.equal(actionMatches("Stats for nerds", "zzz|stats"), true);
  assert.equal(actionMatches("Stats for nerds", "zzz|qqq"), false);
  assert.equal(actionMatches("Stats for nerds", ""), true);
});
