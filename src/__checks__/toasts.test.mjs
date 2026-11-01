import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { MAX_TOASTS, addToast } from "../logic/toasts.ts";
import { NO_PLAYBACK } from "../logic/web.ts";

const toast = (id, text, kind = "info") => ({ id, text, kind });

test("the same words said again while still showing are said once", () => {
  // A double-click on a channel in a browser: click, click, double-click,
  // each saying that nothing plays here.
  let shown = [];
  for (let id = 1; id <= 3; id++) shown = addToast(shown, toast(id, NO_PLAYBACK));
  assert.deepEqual(shown, [toast(1, NO_PLAYBACK)]);
  // Unchanged means the same list, so nothing re-renders and the toast already up
  // goes away when it was going to.
  assert.equal(addToast(shown, toast(4, NO_PLAYBACK)), shown);
  // The same words as an error are a different thing to say.
  assert.equal(addToast(shown, toast(5, NO_PLAYBACK, "error")).length, 2);
});

test("different things are all said, the oldest giving way", () => {
  let shown = [];
  for (let id = 1; id <= 5; id++) shown = addToast(shown, toast(id, `message ${id}`));
  assert.equal(shown.length, MAX_TOASTS);
  assert.deepEqual(
    shown.map((t) => t.id),
    [3, 4, 5],
  );
  // Once the first has gone, it may be said again.
  assert.equal(addToast(shown, toast(6, "message 1")).at(-1).id, 6);
});

test("the store's toasts go through addToast", () => {
  const store = readFileSync(new URL("../store.tsx", import.meta.url), "utf8");
  const at = store.indexOf('case "toast"');
  assert.ok(at > 0);
  assert.match(store.slice(at, at + 400), /addToast\(/);
});
