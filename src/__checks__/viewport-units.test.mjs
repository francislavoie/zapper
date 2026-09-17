// The stylesheet may not size anything in viewport units.
//
// `vh` and `vw` do not mean what they look like here. Application zoom is
// real page zoom, and under the CSS-zoom mechanism a viewport unit is
// resolved against the viewport and then scaled again by the zoom — so at
// 150% a `76vh` dialog occupies 114% of the screen and hangs off the bottom
// of it. Measured against a 700px viewport at 150%: a `position: fixed;
// inset: 0` box is 700px, `height: 100%` inside it is 700px, and `100vh` is
// 1050px.
//
// So the rule is: one fixed, full-viewport backdrop knows how big the screen
// is, and everything inside it is a percentage of that. This guards the rule,
// because the failure is invisible at 100% zoom — which is where anybody
// adding a `vh` would be looking.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const css = readFileSync(new URL("../styles.css", import.meta.url), "utf8");

/** The sheet with its comments removed. Viewport units are discussed at
 *  length in the prose above the rules that avoid them, and a check that
 *  tripped on its own explanation would be useless. */
const declarations = css.replace(/\/\*[\s\S]*?\*\//g, "");

test("nothing is sized in vh or vw", () => {
  // A number — decimal, signed, or bare — followed by the unit and then
  // something that is not an identifier character, so `overflow: hidden` and
  // words ending in "vw" are not matches.
  const offenders = [
    ...declarations.matchAll(/(-?\d*\.?\d+)(vh|vw|dvh|dvw|svh|lvh)\b/g),
  ].map((m) => m[0]);
  assert.deepEqual(
    offenders,
    [],
    `use a percentage of the fixed .modal-backdrop instead: ${offenders.join(", ")}`,
  );
});

test("the backdrop is the box that knows the viewport's size", () => {
  // The whole scheme rests on this one rule being fixed and full-bleed, so
  // that a child's `max-height: 100%` means the screen.
  const backdrop = /\.modal-backdrop\s*\{([^}]*)\}/.exec(declarations);
  assert.ok(backdrop, ".modal-backdrop is missing");
  assert.match(backdrop[1], /position:\s*fixed/, ".modal-backdrop must be fixed");
  assert.match(backdrop[1], /inset:\s*0/, ".modal-backdrop must cover the viewport");
  // Without border-box the padding is added to the child's 100% and the
  // dialog is exactly one padding too tall.
  assert.match(backdrop[1], /box-sizing:\s*border-box/, "padding would escape the cap");
});

test("the dialogs cap themselves against it", () => {
  for (const selector of [".palette", ".settings"]) {
    const rule = new RegExp(`\\${selector}\\s*\\{([^}]*)\\}`).exec(declarations);
    assert.ok(rule, `${selector} is missing`);
    assert.match(rule[1], /max-height:\s*100%/, `${selector} must cap at the screen`);
  }
});
