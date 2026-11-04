// The Movies list's keys: where the selection goes.
// Run: node --test src/__checks__/

import assert from "node:assert/strict";
import { test } from "node:test";
import { movieCursorKey, scrollToShow } from "../logic/movieKeys.ts";

const list = (current, total = 1000) => ({ current, total, columns: 1, visibleRows: 12, posters: false });
const grid = (current, total = 1000) => ({ current, total, columns: 5, visibleRows: 4, posters: true });

test("in the list: one row, a page less one, the ends", () => {
  assert.equal(movieCursorKey("ArrowDown", list(10)), 11);
  assert.equal(movieCursorKey("ArrowUp", list(10)), 9);
  assert.equal(movieCursorKey("PageDown", list(10)), 21);
  assert.equal(movieCursorKey("PageUp", list(10)), 0);
  assert.equal(movieCursorKey("Home", list(500)), 0);
  assert.equal(movieCursorKey("End", list(0)), 999);
});

test("never past either end", () => {
  assert.equal(movieCursorKey("ArrowUp", list(0)), 0);
  assert.equal(movieCursorKey("ArrowDown", list(999)), 999);
  assert.equal(movieCursorKey("PageDown", list(995)), 999);
});

test("from nothing selected the first title is the one that lands", () => {
  assert.equal(movieCursorKey("ArrowDown", list(-1)), 0);
  assert.equal(movieCursorKey("ArrowUp", list(-1)), 0);
  assert.equal(movieCursorKey("End", list(-1)), 999);
});

test("sideways moves belong to the posters; in the list they are left to the player", () => {
  assert.equal(movieCursorKey("ArrowRight", list(3)), null);
  assert.equal(movieCursorKey("ArrowLeft", list(3)), null);
  assert.equal(movieCursorKey("ArrowRight", grid(3)), 4);
  assert.equal(movieCursorKey("ArrowLeft", grid(3)), 2);
});

test("in the posters up and down jump a row, and a page is whole rows", () => {
  assert.equal(movieCursorKey("ArrowDown", grid(7)), 12);
  assert.equal(movieCursorKey("ArrowUp", grid(7)), 2);
  assert.equal(movieCursorKey("PageDown", grid(7)), 22);
  assert.equal(movieCursorKey("ArrowUp", grid(2)), 0);
});

test("other keys, and an empty list, are not movements", () => {
  assert.equal(movieCursorKey("a", list(3)), null);
  assert.equal(movieCursorKey("Enter", list(3)), null);
  assert.equal(movieCursorKey("ArrowDown", list(-1, 0)), null);
});

test("scrolling moves only as far as it must", () => {
  assert.equal(scrollToShow(400, 40, 0, 300), 140, "below: the row's foot meets the edge");
  assert.equal(scrollToShow(40, 40, 200, 300), 40, "above: the row's head meets the edge");
  assert.equal(scrollToShow(250, 40, 200, 300), 200, "in view: untouched");
  assert.equal(scrollToShow(0, 40, 0, 20), 20, "a viewport shorter than the row");
});
