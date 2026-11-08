// The Movies and Shows search's rule, as the mock catalogues run it: the start
// of a word, case and accents aside. The app's own is in Rust
// (src-tauri/src/catalog/title_search.rs), with the same cases as tests.
// Run: node --test src/__checks__/

import assert from "node:assert/strict";
import { test } from "node:test";
import { foldTitle, titleMatches, titleNeedles } from "../logic/titleMatch.ts";

const matches = (query, name) => titleMatches(titleNeedles(query), name);

test("a language prefix matches only where a word starts", () => {
  assert.equal(matches("EN -", "EN - A Movie"), true);
  assert.equal(matches("EN -", "Garden - A Movie"), false);
  assert.equal(matches("EN -", "Harbour Lights"), false);
});

test("a slash ends a word", () => {
  assert.equal(matches("EN -", "FR/EN - A Movie"), true);
  assert.equal(matches("fr", "FR/EN - A Movie"), true);
});

test("a phrase matches in order, from a word's start", () => {
  assert.equal(matches("dark kni", "The Dark Knight"), true);
  assert.equal(matches("knight dark", "The Dark Knight"), false);
  assert.equal(matches("ark", "The Dark Knight"), false);
});

test("case, accents and runs of spaces do not count", () => {
  assert.equal(matches("en -", "EN - A Movie"), true);
  assert.equal(matches("éte", "L'Été"), true);
  assert.equal(matches("ete", "L'Été"), true);
  assert.equal(matches("dark   kni", "The  Dark \t Knight"), true);
  assert.equal(foldTitle("  L'Été \t à  la plage "), "l'ete a la plage");
});

test("a pipe is either side, and empty sides are no search", () => {
  assert.equal(matches("harbour|lantern", "The Lantern Bay"), true);
  assert.equal(matches("EN -|FR -", "Garden - X"), false);
  assert.equal(matches("|harbour|", "Harbour Lights"), true);
  assert.deepEqual(titleNeedles(" | "), []);
  assert.equal(matches("", "Anything"), true);
});
