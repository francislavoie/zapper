// What Settings → Account says about the account's connections.

import test from "node:test";
import assert from "node:assert/strict";
import { connectionsLine } from "../logic/connections.ts";

const status = (p) => ({
  max: 1,
  active: 1,
  ours: 1,
  counted: true,
  declared: false,
  expires: null,
  ...p,
});

test("a provider's count is said as it counts", () => {
  assert.equal(connectionsLine(status({})), "1 of 1 in use");
  assert.equal(connectionsLine(status({ max: 3, active: 0, ours: 0 })), "0 of 3 in use");
});

test("an account nobody can count says what is allowed and what is ours", () => {
  const line = connectionsLine(status({ counted: false, declared: true, active: null, max: 2, ours: 1 }));
  assert.equal(line, "2 allowed · 1 in use here");
});

test("no limit at all is said rather than guessed", () => {
  assert.equal(connectionsLine(status({ counted: false, max: null, active: null })), "No limit reported");
});

test("the expiry is a date, through the format helpers", () => {
  const line = connectionsLine(status({ expires: 1_909_000_000 }));
  assert.match(line, /^1 of 1 in use · expires /);
  assert.match(line, /2030/);
});
