// Component modules may export only components.
//
// React Fast Refresh can hot-swap a module whose every export is a
// component. One extra export — a hook, a constant, a helper — and the whole
// file is demoted: editing it reloads the page and throws away the app's
// state, which during UI work is most of the cost of an edit. The failure is
// silent apart from a line in the dev server log that scrolls past, so it is
// checked here instead.
//
// Type-only exports are fine: they are erased before the browser sees them.

import test from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";

const dir = new URL("../components/", import.meta.url);
const files = readdirSync(dir).filter((f) => f.endsWith(".tsx"));

/** Every runtime (non-type) export name a module declares. */
function valueExports(source) {
  // Comments first: this codebase writes prose about its own exports, and a
  // sentence mentioning one is not a declaration of it.
  const code = source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
  const names = [];
  // `export function X`, `export const X`, `export class X`, `export let X`.
  for (const m of code.matchAll(/^export\s+(?:async\s+)?(?:function|const|let|var|class)\s+([A-Za-z0-9_$]+)/gm)) {
    names.push(m[1]);
  }
  // `export { a, b }` — but not `export type { a }`, which is erased.
  for (const m of code.matchAll(/^export\s+\{([^}]*)\}/gm)) {
    for (const part of m[1].split(",")) {
      const name = part.trim().split(/\s+as\s+/).pop()?.trim();
      // `export { type Foo }` is erased too.
      if (name && !part.trim().startsWith("type ")) names.push(name);
    }
  }
  return names;
}

/** React's own rule: a component is named in PascalCase. A SCREAMING_CASE
 *  name is a constant, and a camelCase one is a hook or a helper. */
const isComponentName = (name) => /^[A-Z][A-Za-z0-9]*$/.test(name) && !/^[A-Z0-9_]+$/.test(name);

test("every component module exports only components", () => {
  assert.ok(files.length > 5, "found suspiciously few component files");
  const offenders = [];
  for (const file of files) {
    const source = readFileSync(new URL(file, dir), "utf8");
    for (const name of valueExports(source)) {
      if (!isComponentName(name)) offenders.push(`${file} exports ${name}`);
    }
  }
  assert.deepEqual(
    offenders,
    [],
    "move these out of src/components/ — a hook belongs in src/hooks/, a " +
      "constant in src/logic/, or Fast Refresh is lost for the whole file:\n" +
      offenders.join("\n"),
  );
});

test("the check can actually see an offender", () => {
  // A guard that cannot fail is not a guard. These are the two shapes an
  // offender most often takes.
  assert.deepEqual(valueExports("export function useFeedback() {}"), ["useFeedback"]);
  assert.deepEqual(valueExports("export const FAVOURITE_LIMIT = 40;"), ["FAVOURITE_LIMIT"]);
  assert.equal(isComponentName("useFeedback"), false);
  assert.equal(isComponentName("FAVOURITE_LIMIT"), false);
  assert.equal(isComponentName("VideoFeedback"), true);
  // And that it does not trip on what is allowed.
  assert.deepEqual(valueExports("export type { Feedback } from './x';"), []);
  assert.deepEqual(valueExports("export interface Props { a: number }"), []);
  assert.deepEqual(valueExports("// export const NOPE = 1\nexport function Ok() {}"), ["Ok"]);
});
