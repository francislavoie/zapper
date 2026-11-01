// The shortcut modifier and how shortcuts are written, on a Mac and
// elsewhere (logic/keys.ts). Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  blocksPlainKey,
  chordKeys,
  chordLabel,
  hintLabel,
  isMacPlatform,
  shortcutModifier,
} from "../logic/keys.ts";
import { SHORTCUT_GROUPS } from "../logic/shortcuts.ts";
import { ACTIONS } from "../logic/palette.ts";
import { zoomKeyCommand } from "../logic/zoom.ts";

test("a Mac is recognised from what WebKit and Chromium report", () => {
  assert.equal(isMacPlatform({ platform: "MacIntel" }), true, "WKWebView on Apple Silicon too");
  assert.equal(isMacPlatform({ uaPlatform: "macOS" }), true);
  assert.equal(isMacPlatform({ userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)" }), true);
  assert.equal(isMacPlatform({ platform: "Linux x86_64" }), false);
  assert.equal(isMacPlatform({ platform: "Win32" }), false);
  assert.equal(isMacPlatform({ uaPlatform: "Windows", platform: "MacIntel" }), false, "UA data first");
  assert.equal(isMacPlatform({}), false);
});

test("?platform=mac forces the Mac look, for the mock", () => {
  assert.equal(isMacPlatform({ platform: "Linux x86_64", search: "?platform=mac" }), true);
  assert.equal(isMacPlatform({ platform: "MacIntel", search: "?platform=linux" }), false);
});

test("elsewhere the shortcut modifier is Ctrl, with Meta still accepted", () => {
  assert.equal(shortcutModifier({ ctrlKey: true }, false), true);
  assert.equal(shortcutModifier({ metaKey: true }, false), true);
  assert.equal(shortcutModifier({}, false), false);
});

test("on a Mac it is Command, and Control does not count", () => {
  assert.equal(shortcutModifier({ metaKey: true }, true), true);
  assert.equal(shortcutModifier({ ctrlKey: true }, true), false, "⌃P is not the palette");
  assert.equal(shortcutModifier({ ctrlKey: true, metaKey: true }, true), false, "⌃⌘ is the system's");
});

test("plain-letter shortcuts stand aside for ⌘ and ⌃ on a Mac only", () => {
  assert.equal(blocksPlainKey({ metaKey: true }, true), true);
  assert.equal(blocksPlainKey({ ctrlKey: true }, true), true);
  assert.equal(blocksPlainKey({}, true), false);
  assert.equal(blocksPlainKey({ ctrlKey: true }, false), false, "Linux and Windows unchanged");
});

test("zoom keys follow the shortcut modifier", () => {
  assert.equal(zoomKeyCommand({ key: "=", metaKey: true }, true), "in");
  assert.equal(zoomKeyCommand({ key: "=", ctrlKey: true }, true), null);
  assert.equal(zoomKeyCommand({ key: "0", ctrlKey: true }, false), "reset");
  assert.equal(zoomKeyCommand({ key: "0", ctrlKey: true }), "reset", "elsewhere by default");
});

test("keycaps: unchanged elsewhere, Apple's symbols in Apple's order on a Mac", () => {
  assert.deepEqual(chordKeys(["Ctrl", "P"], false), ["Ctrl", "P"]);
  assert.deepEqual(chordKeys(["Ctrl", "P"], true), ["⌘", "P"]);
  assert.deepEqual(chordKeys(["Shift", "Wheel"], true), ["⇧", "Wheel"]);
  // ⌃⌥⇧⌘, whatever order the data has them in.
  assert.deepEqual(chordKeys(["Ctrl", "Shift", "Alt", "Control", "K"], true), ["⌃", "⌥", "⇧", "⌘", "K"]);
  assert.deepEqual(chordKeys(["Control", "K"], false), ["Ctrl", "K"]);
});

test("labels: Ctrl+P elsewhere, ⌘P on a Mac", () => {
  assert.equal(chordLabel(["Ctrl", "P"], false), "Ctrl+P");
  assert.equal(chordLabel(["Ctrl", "P"], true), "⌘P");
  assert.equal(chordLabel(["Ctrl", "Shift", "F"], true), "⇧⌘F");
  assert.equal(chordLabel(["Ctrl", "0"], true), "⌘0");
  assert.equal(chordLabel(["Ctrl", "+"], true), "⌘+");
  assert.equal(chordLabel(["Ctrl"], true), "⌘");
  assert.equal(chordLabel(["Ctrl"], false), "Ctrl");
  assert.equal(chordLabel(["Alt", "Space"], true), "⌥ Space");
});

test("hints that are chords are translated, anything else is left alone", () => {
  assert.equal(hintLabel("Ctrl+Q", true), "⌘Q");
  assert.equal(hintLabel("Ctrl+Q", false), "Ctrl+Q");
  for (const h of ["Esc", "G", "P · last · recall", "⚙", "", "Space"]) {
    assert.equal(hintLabel(h, true), h);
  }
});

test("no shortcut or palette hint on a Mac still says Ctrl, Alt or Shift", () => {
  const words = /\b(Ctrl|Control|Alt|Shift)\b/;
  for (const g of SHORTCUT_GROUPS) {
    for (const it of g.items) {
      for (const chord of it.keys) {
        assert.doesNotMatch(chordKeys(chord, true).join(" "), words, it.does);
      }
    }
  }
  for (const a of ACTIONS) assert.doesNotMatch(hintLabel(a.hint ?? "", true), words, a.label);
});
