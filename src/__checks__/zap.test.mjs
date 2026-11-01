// Tuning by number: what a run of keystrokes means, and when it is finished.
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  ZAP_MAX_TYPED,
  afterKey,
  firstDecision,
  buildNumberIndex,
  numberKey,
  resolve,
  settle,
  typeChar,
  zapCells,
  zapChar,
} from "../logic/zap.ts";

/** A numbered catalogue: 1–120, plus a duplicate 12 that the list draws
 *  second, plus two over-the-air subchannels. */
const rows = [
  ...Array.from({ length: 120 }, (_, i) => [String(i + 1), 1000 + i + 1]),
  ["12", 9012],
  ["5.1", 5001],
  ["5.2", 5002],
];
const index = buildNumberIndex(rows);

const type = (keys) => [...keys].reduce(typeChar, "");

test("the index knows its widest number and keeps the list's first of a pair", () => {
  assert.equal(index.longest, 3);
  assert.equal(index.first.get("12"), 1012, "the first 12 in list order");
  assert.equal(index.first.get("5.1"), 5001);
});

test("digits and a point are the only keys an entry takes", () => {
  assert.equal(zapChar({ key: "7" }), "7");
  assert.equal(zapChar({ key: "." }), ".");
  // The keypad's decimal key prints a comma in some locales; it is still the
  // point, because that is the key a remote's dot is on.
  assert.equal(zapChar({ key: ",", code: "NumpadDecimal" }), ".");
  assert.equal(zapChar({ key: "a" }), null);
  assert.equal(zapChar({ key: "Enter" }), null);
  assert.equal(zapChar({ key: "F1" }), null);
  // Ctrl+0 is the zoom reset; Alt and Meta digits belong to the system.
  assert.equal(zapChar({ key: "0", ctrlKey: true }), null);
  assert.equal(zapChar({ key: "1", altKey: true }), null);
  assert.equal(zapChar({ key: "1", metaKey: true }), null);
});

test("a point goes after a digit, and only once", () => {
  assert.equal(typeChar("", "."), "");
  assert.equal(typeChar("5", "."), "5.");
  assert.equal(typeChar("5.", "."), "5.");
  assert.equal(typeChar("5.1", "."), "5.1");
  assert.equal(typeChar("1", "x"), "1");
  assert.equal(type("1".repeat(20)).length, ZAP_MAX_TYPED);
});

test("leading zeros are a remote's fixed width, not part of the number", () => {
  assert.equal(numberKey("007"), "7");
  assert.equal(numberKey("0"), "0");
  assert.equal(numberKey("000"), "0");
  assert.equal(numberKey("05.1"), "5.1");
  assert.equal(numberKey("0.1"), "0.1");
});

test("a number that cannot go on tunes at once", () => {
  // 13 is a channel and nothing longer starts with 13 among 1–120.
  assert.deepEqual(afterKey(index, "13"), { kind: "tune", streamId: 1013, label: "13" });
  // Three digits is the widest number there is.
  assert.deepEqual(afterKey(index, "100"), { kind: "tune", streamId: 1100, label: "100" });
  assert.deepEqual(afterKey(index, "007"), { kind: "tune", streamId: 1007, label: "7" });
});

test("a number that could still grow waits", () => {
  // 1, 10–19, 100–120 all start with 1.
  assert.deepEqual(afterKey(index, "1"), { kind: "wait" });
  assert.deepEqual(afterKey(index, "12"), { kind: "wait" }, "120 is still possible");
  assert.deepEqual(afterKey(index, "0"), { kind: "wait" });
  assert.deepEqual(afterKey(index, "00"), { kind: "wait" });
  assert.deepEqual(afterKey(index, "5"), { kind: "wait" }, "5.1 and 50–59 follow");
  assert.deepEqual(afterKey(index, "5."), { kind: "wait" });
});

test("a number nothing starts with is a miss straight away", () => {
  assert.deepEqual(afterKey(index, "13"), { kind: "tune", streamId: 1013, label: "13" });
  assert.deepEqual(afterKey(index, "121"), { kind: "miss", label: "121" });
  assert.deepEqual(afterKey(index, "5.3"), { kind: "miss", label: "5.3" });
  assert.deepEqual(afterKey(index, "000"), { kind: "miss", label: "0" }, "no channel 0");
});

test("Enter or a pause takes what is typed as it stands", () => {
  assert.deepEqual(settle(index, "1"), { kind: "tune", streamId: 1001, label: "1" });
  assert.deepEqual(settle(index, "12"), { kind: "tune", streamId: 1012, label: "12" });
  assert.deepEqual(settle(index, "5.1"), { kind: "tune", streamId: 5001, label: "5.1" });
  assert.deepEqual(settle(index, "5."), { kind: "miss", label: "5." });
  assert.deepEqual(settle(index, "0"), { kind: "miss", label: "0" });
});

test("subchannels tune once the dot and digit are in", () => {
  const ota = buildNumberIndex([["2.1", 1], ["5.1", 2], ["5.2", 3], ["13.1", 4]]);
  assert.equal(ota.longest, 4);
  assert.deepEqual(afterKey(ota, "5"), { kind: "wait" });
  assert.deepEqual(afterKey(ota, "5.2"), { kind: "tune", streamId: 3, label: "5.2" });
  // There is no channel 5 on its own, so a pause on it is a miss.
  assert.deepEqual(settle(ota, "5"), { kind: "miss", label: "5" });
});

test("the blanks show how long the number could still be", () => {
  assert.equal(resolve(index, "").slots, 3);
  assert.equal(resolve(index, "1").slots, 3);
  assert.equal(resolve(index, "13").slots, 2, "nothing longer starts with 13");
  assert.equal(resolve(index, "5.").slots, 3);
  assert.deepEqual(zapCells("12", 3), ["1", "2", "_"]);
  assert.deepEqual(zapCells("13", 2), ["1", "3"]);
  // A run of zeros never claims more room than the widest number.
  assert.equal(resolve(index, "0").slots, 3);
  assert.equal(resolve(index, "0000").slots, 4);
});

test("an empty catalogue misses everything rather than waiting forever", () => {
  const none = buildNumberIndex([]);
  assert.deepEqual(afterKey(none, "4"), { kind: "miss", label: "4" });
  assert.deepEqual(afterKey(none, "0"), { kind: "miss", label: "0" });
});

test("keys typed before the numbers arrived are decided as if they had not been", () => {
  const big = buildNumberIndex(Array.from({ length: 60000 }, (_, i) => [String(i + 1), i + 1]));
  assert.deepEqual(firstDecision(big, "70001"), {
    decision: { kind: "tune", streamId: 7000, label: "7000" },
    rest: "1",
  });
  assert.deepEqual(firstDecision(big, "12"), { decision: { kind: "wait" }, rest: "" });
  assert.deepEqual(firstDecision(index, "13.5"), {
    decision: { kind: "tune", streamId: 1013, label: "13" },
    rest: "5",
  });
  assert.deepEqual(firstDecision(index, "121"), {
    decision: { kind: "miss", label: "121" },
    rest: "",
  });
});
