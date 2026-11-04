// The Movies list's page cache: what a response may and may not do.
// Run: node --test src/__checks__/

import assert from "node:assert/strict";
import { test } from "node:test";
import { PageBook } from "../logic/pagedRows.ts";
import { PAGE_SIZE } from "../logic/virtual.ts";

const rows = (from, n) => Array.from({ length: n }, (_, i) => from + i);

test("the first thing asked for is page 0, whatever the window says", () => {
  const b = new PageBook();
  b.sync("a", 0);
  assert.deepEqual(b.want(0, 0), [0]);
  assert.equal(b.begin(b.ticket, 0), true);
  assert.deepEqual(b.want(0, 0), [], "in flight is not asked again");
});

test("the total is the first answer's, exactly", () => {
  const b = new PageBook();
  b.sync("a", 0);
  b.begin(b.ticket, 0);
  assert.equal(b.accept(b.ticket, 0, 1234, rows(0, PAGE_SIZE)), true);
  assert.equal(b.total, 1234);
  assert.equal(b.rowAt(5), 5);
  assert.equal(b.rowAt(1234), undefined);
  assert.equal(b.rowAt(PAGE_SIZE), undefined, "not loaded yet");
  assert.deepEqual(b.want(PAGE_SIZE - 5, PAGE_SIZE + 5), [1]);
});

test("a window over pages already held, in flight or failed asks for nothing", () => {
  const b = new PageBook();
  b.sync("a", 0);
  const t = b.ticket;
  b.begin(t, 0);
  b.accept(t, 0, 1000, rows(0, PAGE_SIZE));
  b.begin(t, 1);
  b.begin(t, 2);
  b.fail(t, 2, "offline");
  assert.deepEqual(b.want(0, 3 * PAGE_SIZE), []);
  assert.equal(b.error, "offline");
  b.retry();
  assert.deepEqual(b.want(0, 3 * PAGE_SIZE), [2]);
});

test("an answer to another list never lands in this one", () => {
  const b = new PageBook();
  b.sync("sort=az", 0);
  const old = b.ticket;
  b.begin(old, 0);
  b.sync("sort=year", 0);
  assert.equal(b.accept(old, 0, 999, rows(100, PAGE_SIZE)), false);
  assert.equal(b.total, 0);
  assert.equal(b.rowAt(0), undefined);
  assert.equal(b.fail(old, 0, "late"), false);
  assert.equal(b.error, null);
  // The new list asks for its own page 0, though the old one was in flight.
  assert.equal(b.begin(b.ticket, 0), true);
});

test("an answer from before a refresh is dropped, and the old rows stay until replaced", () => {
  const b = new PageBook();
  b.sync("a", 0);
  const first = b.ticket;
  b.begin(first, 0);
  b.accept(first, 0, 300, rows(0, PAGE_SIZE));
  b.begin(first, 1);
  b.sync("a", 1);
  assert.equal(b.accept(first, 1, 300, rows(900, PAGE_SIZE)), false);
  // Still showing the old page and length while the refresh is under way.
  assert.equal(b.total, 300);
  assert.equal(b.rowAt(7), 7);
  assert.deepEqual(b.want(0, 10), [0]);
  b.begin(b.ticket, 0);
  b.accept(b.ticket, 0, 120, rows(500, PAGE_SIZE));
  assert.equal(b.total, 120);
  assert.equal(b.rowAt(7), 507);
  assert.equal(b.rowAt(PAGE_SIZE), undefined, "past the new end");
});

test("a refresh scrolled far down learns the new length from page 0", () => {
  const b = new PageBook();
  b.sync("a", 0);
  b.begin(b.ticket, 0);
  b.accept(b.ticket, 0, 5000, rows(0, PAGE_SIZE));
  b.sync("a", 1);
  assert.deepEqual(b.want(4000, 4040), [0, 20]);
});

test("another list starts empty, not showing the last one's rows", () => {
  const b = new PageBook();
  b.sync("a", 0);
  b.begin(b.ticket, 0);
  b.accept(b.ticket, 0, 50, rows(0, 50));
  b.sync("b", 0);
  assert.equal(b.total, 0);
  assert.equal(b.rowAt(0), undefined);
});
