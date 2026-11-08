// Tab kept inside a modal dialog: logic/focusTrap.ts.

import test from "node:test";
import assert from "node:assert/strict";
import { trapStep } from "../logic/focusTrap.ts";

test("Tab past the last control goes back to the first, Shift+Tab the other way", () => {
  assert.equal(trapStep(3, 2, false), 0);
  assert.equal(trapStep(3, 0, true), 2);
});

test("within the dialog Tab moves one control either way", () => {
  assert.equal(trapStep(3, 0, false), 1);
  assert.equal(trapStep(3, 1, false), 2);
  assert.equal(trapStep(3, 2, true), 1);
});

test("from outside the controls Tab goes to the first, Shift+Tab to the last", () => {
  assert.equal(trapStep(3, -1, false), 0);
  assert.equal(trapStep(3, -1, true), 2);
});

test("one control keeps the focus; none leaves it where it is", () => {
  assert.equal(trapStep(1, 0, false), 0);
  assert.equal(trapStep(1, 0, true), 0);
  assert.equal(trapStep(0, -1, false), null);
});
