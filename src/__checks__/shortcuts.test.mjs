// The shortcut list in Settings → Keyboard against the keys the app actually
// handles. Prose drifts from the handlers; this makes drift a failure.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { MOUSE_WORDS, SHORTCUT_GROUPS } from "../logic/shortcuts.ts";

// Number zapping has a hook of its own, listening ahead of everything else
// while a number is being typed; its keys count as handled too.
const keysSource =
  readFileSync(new URL("../hooks/useGlobalKeys.ts", import.meta.url), "utf8") +
  readFileSync(new URL("../hooks/useNumberZap.ts", import.meta.url), "utf8");
const zoomSource = readFileSync(new URL("../logic/zoom.ts", import.meta.url), "utf8");

/** How a listed key appears in the handler's source. */
const IN_SOURCE = {
  Space: '" "',
  "←": '"ArrowLeft"',
  "→": '"ArrowRight"',
  Esc: '"Escape"',
  Home: '"Home"',
  "/": '"/"',
  // Digits are recognised by zapChar in logic/zap.ts, which the hook calls.
  "0–9": "zapChar(",
};

test("every key listed is one the app handles", () => {
  for (const g of SHORTCUT_GROUPS) {
    for (const it of g.items) {
      for (const chord of it.keys) {
        for (const k of chord) {
          if (MOUSE_WORDS.has(k) || k === "Ctrl" || k === "Shift") continue;
          if (["+", "−", "0"].includes(k)) {
            // Zoom keys live in zoom.ts.
            assert.match(zoomSource, k === "0" ? /"0"/ : k === "+" ? /"\+"/ : /"-"/, `zoom key ${k}`);
            continue;
          }
          const needle = IN_SOURCE[k] ?? `"${k.toLowerCase()}"`;
          assert.ok(keysSource.includes(needle), `"${k}" (${it.does}) is not handled in useGlobalKeys.ts`);
        }
      }
    }
  }
});

test("every letter shortcut the app handles is listed", () => {
  const handled = [...keysSource.matchAll(/e\.key === "([a-z])"|case "([a-z])":/g)].map(
    (m) => (m[1] ?? m[2]).toUpperCase(),
  );
  const listed = new Set(
    SHORTCUT_GROUPS.flatMap((g) => g.items.flatMap((it) => it.keys.flat())),
  );
  for (const k of new Set(handled)) assert.ok(listed.has(k), `${k} is handled but not listed`);
});

test("rows read cleanly: something to press and something it does", () => {
  for (const g of SHORTCUT_GROUPS) {
    assert.ok(g.items.length > 0, g.title);
    for (const it of g.items) {
      assert.ok(it.keys.length > 0 && it.keys.every((c) => c.length > 0), it.does);
      assert.ok(it.does.length > 0 && it.does.length <= 60, `too long: ${it.does}`);
    }
  }
});
