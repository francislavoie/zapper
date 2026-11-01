// No native tooltips.
//
// The `title` attribute's tooltip waits most of a second before appearing,
// is drawn by the toolkit in its own light theme over a dark app, and cannot
// be told to appear only when text is actually cut off. This app has its own
// (src/tooltip.ts: `data-tip`, and `data-tip-overflow` for ellipsized text),
// and the rule is that nothing reaches for `title` instead.
//
// It is easy to break without noticing, because `title` is the first thing
// anybody types for a tooltip and it works — just slowly, and in the wrong
// colours. So it is checked here rather than left to memory.
//
// What counts: a `title=` attribute on any JSX element or component (a
// component's `title` prop is where a native tooltip comes from soon enough,
// and a prop that is really a heading can be called one), and the DOM
// spellings — `el.title = …` and `setAttribute("title", …)`. What does not:
// an SVG <title> child, which names an icon for assistive technology, and
// `title:` as a data field (a programme has a title).

import test from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));

/** Files still to be converted, by path relative to src/. Each entry is a
 *  debt, not an exemption: delete it once the file is clean. Empty: every
 *  file is converted. */
const PENDING = new Set([]);

function sources(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((d) => {
    const path = join(dir, d.name);
    if (d.isDirectory()) return d.name === "__checks__" ? [] : sources(path);
    return /\.(tsx?|jsx?)$/.test(d.name) ? [path] : [];
  });
}

/** Code with its comments removed, line breaks kept so line numbers hold.
 *  This codebase writes prose about what it avoids, and a comment that says
 *  "not title=" is not a use of it. */
function code(source) {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, (c) => c.replace(/[^\n]/g, " "))
    .replace(/^\s*\/\/.*$/gm, "");
}

const PATTERNS = [
  // `title="…"`, `title={…}`, `title='…'` — but not `subtitle=`, `data-title=`, or
  // a property assignment (the next pattern).
  /(?<![\w.-])title\s*=\s*["'{]/g,
  // `el.title = …`, but not `document.title`, which is the window's name.
  /(?<!document)\.title\s*=(?!=)/g,
  /setAttribute\(\s*["']title["']/g,
];

function offences(file) {
  const text = code(readFileSync(file, "utf8"));
  const found = [];
  for (const pattern of PATTERNS) {
    for (const m of text.matchAll(pattern)) {
      const line = text.slice(0, m.index).split("\n").length;
      found.push(`${relative(root, file)}:${line}: ${m[0]}`);
    }
  }
  return found;
}

test("nothing uses a native title tooltip", () => {
  const found = sources(root)
    .filter((f) => !PENDING.has(relative(root, f).split("\\").join("/")))
    .flatMap(offences);
  assert.deepEqual(found, [], `use data-tip / data-tip-overflow instead:\n${found.join("\n")}`);
});

test("the check itself catches each spelling", () => {
  // Guards the guard: a pattern that matched nothing would pass forever.
  const hits = (s) => PATTERNS.some((p) => new RegExp(p.source).test(code(s)));
  assert.equal(hits(`<button title="Stop">`), true);
  assert.equal(hits(`<span title={name}>`), true);
  assert.equal(hits(`<ListHead\n  title={x}`), true);
  assert.equal(hits(`el.title = "x";`), true);
  assert.equal(hits(`el.setAttribute("title", x)`), true);

  assert.equal(hits(`<title>{title}</title>`), false);
  assert.equal(hits(`{ title: pick(PROGRAMMES) }`), false);
  assert.equal(hits(`<Sub subtitle={s} />`), false);
  assert.equal(hits(`<b data-title="x" />`), false);
  assert.equal(hits(`document.title = "zapper";`), false);
  assert.equal(hits(`// never title="x"`), false);
  assert.equal(hits(`/* not title={x} */`), false);
});

test("a pending file is still pending", () => {
  // When the last `title` leaves a pending file, its entry must go too, or
  // the file could quietly take one back.
  for (const rel of PENDING) {
    assert.notDeepEqual(
      offences(join(root, rel)),
      [],
      `${rel} is clean now: remove it from PENDING`,
    );
  }
});
