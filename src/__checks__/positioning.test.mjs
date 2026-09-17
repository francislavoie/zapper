// Rules that only work because another rule exists.
//
// An absolutely positioned box is placed against its nearest *positioned*
// ancestor. When that ancestor's `position` is only there to be that anchor,
// nothing in the file says so, and deleting it moves the child to somewhere
// else entirely — still styled, still revealed on hover, several hundred
// pixels from the thing it belongs to. Moving a handle out of the docked
// picture, say, can take its `position: relative` along, and the hover
// controls with it.
//
// So each pairing is written down here.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const css = readFileSync(new URL("../styles.css", import.meta.url), "utf8").replace(
  /\/\*[\s\S]*?\*\//g,
  "",
);

function ruleFor(selector) {
  const m = new RegExp(`^\\${selector}\\s*\\{([^}]*)\\}`, "m").exec(css);
  assert.ok(m, `${selector} is missing from the stylesheet`);
  return m[1];
}

/** Every anchor, and the absolutely positioned thing that needs it. */
const ANCHORS = [
  [".watch-video", ".mini-controls", "the hover controls on the docked picture"],
  [".rail", ".rail-grip", "the divider between the rail and the list"],
  [".rail-spacer", ".rail-item", "the windowed category rows"],
  [".scrubber", ".scrub-hint", "the scrubber's hover preview"],
];

test("anything absolute has an ancestor positioned for it", () => {
  for (const [anchor, child, what] of ANCHORS) {
    assert.match(
      ruleFor(anchor),
      /position:\s*(relative|absolute|fixed|sticky)/,
      `${anchor} must stay positioned — ${child} is placed against it (${what})`,
    );
    assert.match(
      ruleFor(child),
      /position:\s*absolute/,
      `${child} is expected to be absolute; if that changed, this pairing is stale`,
    );
  }
});

test("the modal backdrop is the box dialogs measure themselves against", () => {
  // Same class of dependency: `max-height: 100%` on a dialog means the
  // screen only because the backdrop is fixed and full-bleed.
  assert.match(ruleFor(".modal-backdrop"), /position:\s*fixed/);
  assert.match(ruleFor(".modal-backdrop"), /inset:\s*0/);
});
