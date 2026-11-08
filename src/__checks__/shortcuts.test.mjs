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
  readFileSync(new URL("../hooks/useNumberZap.ts", import.meta.url), "utf8") +
  // The Movies list's own keys: what each does is in logic/movieKeys.ts.
  readFileSync(new URL("../logic/movieKeys.ts", import.meta.url), "utf8") +
  readFileSync(new URL("../components/MoviesList.tsx", import.meta.url), "utf8");
const zoomSource = readFileSync(new URL("../logic/zoom.ts", import.meta.url), "utf8");

/** How a listed key appears in the handler's source. */
const IN_SOURCE = {
  Space: '" "',
  "←": '"ArrowLeft"',
  "→": '"ArrowRight"',
  Esc: '"Escape"',
  Home: '"Home"',
  "/": '"/"',
  "↑": '"ArrowUp"',
  "↓": '"ArrowDown"',
  PageUp: '"PageUp"',
  PageDown: '"PageDown"',
  End: '"End"',
  Enter: '"Enter"',
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

test("a browser lists the player's keys but not mpv's or the window's, and the app lists them all", async () => {
  const { shortcutGroups } = await import("../logic/shortcuts.ts");
  const { BROWSER, DESKTOP } = await import("../logic/capabilities.ts");
  const count = (groups) => groups.reduce((n, g) => n + g.items.length, 0);
  assert.equal(count(shortcutGroups(DESKTOP)), count(SHORTCUT_GROUPS));
  const browser = shortcutGroups(BROWSER);
  const titles = browser.map((g) => g.title);
  assert.ok(titles.includes("While watching"), titles.join(", "));
  assert.ok(titles.includes("In the guide"));
  const does = browser.flatMap((g) => g.items.map((it) => it.does)).join("\n");
  for (const gone of ["Quit zapper", "Stats for nerds"]) {
    assert.ok(!does.includes(gone), gone);
  }
  for (const kept of [
    "Command palette",
    "Search channels",
    "An hour earlier or later",
    "Previous channel",
    "volume",
    "Play or pause",
  ]) {
    assert.ok(does.includes(kept), kept);
  }
  assert.ok(browser.every((g) => g.items.length > 0));

  // A browser that can't play (no Media Source Extensions) lists none of them.
  const cannot = shortcutGroups({ ...BROWSER, playback: false });
  assert.ok(!cannot.map((g) => g.title).includes("While watching"));
  const cannotDo = cannot.flatMap((g) => g.items.map((it) => it.does)).join("\n");
  for (const gone of ["Previous channel", "volume", "the picture"]) {
    assert.ok(!cannotDo.includes(gone), gone);
  }
});

test("the arrows' step names both kinds of title", () => {
  const does = SHORTCUT_GROUPS.flatMap((g) => g.items)
    .filter((it) => it.keys.some((c) => c[0] === "←"))
    .map((it) => it.does);
  assert.ok(does.includes("Back or forward 5 seconds (10 in a movie or episode)"), does.join("; "));
});
