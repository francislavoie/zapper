// Run: node --test src/__checks__/

import assert from "node:assert/strict";
import { test } from "node:test";
import { createPending } from "../logic/pending.ts";

test("a second claim on the same key is refused until it is released", () => {
  const p = createPending();
  assert.equal(p.claim("movie:1"), true);
  assert.equal(p.claim("movie:1"), false);
  p.release("movie:1");
  assert.equal(p.claim("movie:1"), true);
});
test("other keys are independent", () => {
  const p = createPending();
  assert.equal(p.claim("movie:1"), true);
  assert.equal(p.claim("episode:1"), true);
});
