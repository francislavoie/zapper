// Chrome does not drag-select.
//
// This is a stylesheet rule, not a component one, so it is checked against the
// stylesheet: every regression here would arrive as somebody adding a selector
// to the allowlist, or as the one `user-select: none` at the root quietly
// going away. Both are visible in the text.
//
// The computed value is a browser question and is asserted by driving the live
// page; what a file on disk can prove is that the rule is there, that the
// allowlist is short, and that no piece of ordinary chrome has crept into it.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const css = readFileSync(new URL("../styles.css", import.meta.url), "utf8");

/** Every rule in the sheet as { selectors, declarations }. Comments go first
 *  so a selector mentioned in prose is not mistaken for a real one — this app
 *  comments heavily, and half of these class names appear in sentences. */
function rules(source) {
  const text = source.replace(/\/\*[\s\S]*?\*\//g, "");
  const out = [];
  // At-rules with nested blocks (@media, @keyframes) are skipped: nothing in
  // this check lives inside one, and pretending to parse them would be a lie
  // about how much of the sheet this understands.
  const body = text.replace(/@[a-z-]+[^{]*\{(?:[^{}]*\{[^{}]*\})*[^{}]*\}/gi, "");
  const re = /([^{}]+)\{([^{}]*)\}/g;
  let m;
  while ((m = re.exec(body)) !== null) {
    const selectors = m[1]
      .split(",")
      .map((s) => s.trim().replace(/\s+/g, " "))
      .filter((s) => s !== "");
    const declarations = new Map(
      m[2]
        .split(";")
        .map((d) => d.split(":"))
        .filter((p) => p.length >= 2)
        .map((p) => [p[0].trim(), p.slice(1).join(":").trim()]),
    );
    if (selectors.length > 0) out.push({ selectors, declarations });
  }
  return out;
}

const SHEET = rules(css);

const ruleFor = (selector) =>
  SHEET.find((r) => r.selectors.includes(selector)) ?? null;

/** Every selector anywhere in the sheet that switches selection back on. */
const selectableSelectors = SHEET.filter(
  (r) => (r.declarations.get("user-select") ?? "none") !== "none",
).flatMap((r) => r.selectors);

test("the sheet parses into something recognisable as this app's styles", () => {
  assert.ok(SHEET.length > 50, `only ${SHEET.length} rules parsed`);
  assert.ok(ruleFor(".app"), ".app must be a rule, not just a mention");
  assert.ok(ruleFor(".toolbar"));
});

test("the app root turns selection off, with the prefix older engines need", () => {
  const app = ruleFor(".app").declarations;
  assert.equal(app.get("user-select"), "none");
  assert.equal(app.get("-webkit-user-select"), "none");
});

test("selection comes back only where it is the point", () => {
  // Anything typed into, anything worth pasting back to somebody, and an
  // explicit opt-out attribute for the next case that earns it.
  const expected = [
    ".app input",
    ".app textarea",
    ".app [contenteditable]",
    ".app .toast",
    ".app .pane-error",
    ".app .settings-note",
    ".app .measured",
    ".app .watch-state",
    ".app .watch-col.facts dd",
    ".app [data-selectable]",
  ];
  assert.deepEqual(new Set(selectableSelectors), new Set(expected));
  for (const s of expected) {
    assert.equal(ruleFor(s).declarations.get("user-select"), "text", s);
    assert.equal(ruleFor(s).declarations.get("-webkit-user-select"), "text", s);
  }
});

test("ordinary chrome is not on the allowlist, by class or by ancestor", () => {
  // A sample across every pane: the toolbar and its menu, the rail, the list,
  // the band, the guide. None of these is text anybody copies, and all of them
  // are things people drag across.
  const chrome = [
    "tb-status-text",
    "tb-menu-label",
    "tb-menu-hint",
    "rail-item",
    "rail-name",
    "rail-count",
    "rail-tab",
    "fav-item",
    "fav-name",
    "fav-meta",
    "ctx-head",
    "chrome-back",
    "vfb-label",
    "row-name",
    "row-num",
    "row-now",
    "badge",
    "watch-title",
    "watch-video-empty",
    "watch-when",
    "guide-name",
    "guide-num",
    "prog-title",
    "prog-time",
    "palette-label",
    "list-meta",
    "sticky-head",
  ];
  for (const cls of chrome) {
    const hit = selectableSelectors.find((s) =>
      s.split(" ").some((compound) => compound.split(/(?=[.:#\[])/).includes(`.${cls}`)),
    );
    assert.equal(hit, undefined, `.${cls} must not be selectable (matched "${hit}")`);
  }
});

test("images cannot be peeled off and dropped somewhere", () => {
  // Channel logos are real <img> elements; without this a drag hands the
  // desktop a ghost of the logo, which is a thing no application does.
  const img = ruleFor("img");
  assert.ok(img, "there must be a rule for img");
  assert.equal(img.declarations.get("-webkit-user-drag"), "none");
  assert.equal(img.declarations.get("user-select"), "none");
});

test("the drag handle does not select the status text it contains", () => {
  // The toolbar's centre is the only way to move the window. A drag starting
  // on its text must move the window, not highlight the words.
  const drag = ruleFor(".tb-drag").declarations;
  assert.equal(drag.get("user-select"), "none");
  assert.equal(drag.get("-webkit-user-select"), "none");
});
