// The Continue watching slice of the store: the list, what is open on it, and
// the question a held open asks. Run: node --test src/__checks__/

import assert from "node:assert/strict";
import { test } from "node:test";
import { continueReducer, initialContinue, isContinueAction } from "../continueState.ts";
import { failed, loading, ready } from "../logic/async.ts";

const movie = (id, position = 600) => ({
  kind: "movie",
  lastWatched: 100,
  movie: {
    movieId: id, name: `Film ${id}`, cleanName: `Film ${id}`, year: 2020, rating: 7, posterUrl: null,
    added: null, durationSecs: 5400, categoryId: "c1", favourite: false, watched: false,
    position, duration: 5400, playedAt: 100,
  },
});
const key = (id) => `movie:${id}`;

test("starts empty and idle", () => {
  const s = initialContinue();
  assert.equal(s.items.status, "idle");
  assert.equal(s.selected, null);
  assert.equal(s.menu, null);
  assert.equal(s.prompt, null);
  assert.equal(s.revision, 0);
});
test("a loading or failed refresh never blanks a ready list", () => {
  let s = continueReducer(initialContinue(), { type: "continue/items", value: ready([movie(1)]) });
  s = continueReducer(s, { type: "continue/items", value: loading() });
  assert.equal(s.items.status, "ready");
  s = continueReducer(s, { type: "continue/items", value: failed("offline") });
  assert.equal(s.items.status, "ready");
  assert.equal(s.items.data.length, 1);
});
test("a loading or failed first read is shown", () => {
  let s = continueReducer(initialContinue(), { type: "continue/items", value: loading() });
  assert.equal(s.items.status, "loading");
  s = continueReducer(s, { type: "continue/items", value: failed("offline") });
  assert.equal(s.items.status, "error");
});
test("a fresh list replaces the selection with its new copy, by key", () => {
  let s = continueReducer(initialContinue(), { type: "continue/items", value: ready([movie(1, 600)]) });
  s = continueReducer(s, { type: "continue/select", item: s.items.data[0] });
  s = continueReducer(s, { type: "continue/items", value: ready([movie(2), movie(1, 900)]) });
  assert.equal(s.selected.movie.position, 900);
  assert.equal(s.selected.movie.movieId, 1);
});
test("a fresh list without the selection's key drops it", () => {
  let s = continueReducer(initialContinue(), { type: "continue/items", value: ready([movie(1)]) });
  s = continueReducer(s, { type: "continue/select", item: s.items.data[0] });
  s = continueReducer(s, { type: "continue/items", value: ready([movie(2)]) });
  assert.equal(s.selected, null);
});
test("the menu opens and closes", () => {
  let s = continueReducer(initialContinue(), {
    type: "continue/menu",
    menu: { key: key(1), x: 3, y: 4, takeFocus: true },
  });
  assert.equal(s.menu.key, key(1));
  s = continueReducer(s, { type: "continue/menu", menu: null });
  assert.equal(s.menu, null);
});
test("the prompt is stored and cleared", () => {
  const prompt = { item: { kind: "movie", id: 1 }, title: "Film 1", position: 600, playedAt: 5 };
  let s = continueReducer(initialContinue(), { type: "continue/prompt", prompt });
  assert.equal(s.prompt.title, "Film 1");
  s = continueReducer(s, { type: "continue/prompt", prompt: null });
  assert.equal(s.prompt, null);
});
test("changed bumps the revision", () => {
  const s = continueReducer(initialContinue(), { type: "continue/changed" });
  assert.equal(s.revision, 1);
});
test("reset (another account) clears the list, selection and menu, and bumps the revision", () => {
  let s = continueReducer(initialContinue(), { type: "continue/items", value: ready([movie(1)]) });
  s = continueReducer(s, { type: "continue/select", item: s.items.data[0] });
  s = continueReducer(s, { type: "continue/menu", menu: { key: key(1), x: 0, y: 0, takeFocus: false } });
  s = continueReducer(s, { type: "continue/reset" });
  assert.equal(s.items.status, "idle");
  assert.equal(s.selected, null);
  assert.equal(s.menu, null);
  assert.equal(s.revision, 1);
});
test("isContinueAction recognises its own", () => {
  assert.equal(isContinueAction({ type: "continue/changed" }), true);
  assert.equal(isContinueAction({ type: "movies/select" }), false);
});
