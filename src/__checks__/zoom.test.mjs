// Application zoom is two pieces of arithmetic and one matcher: which stop a
// step lands on, what root font size that means, and whether a given keystroke
// was a zoom at all. Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  ZOOM_BASE_PX,
  ZOOM_DEFAULT,
  ZOOM_MAX,
  ZOOM_MIN,
  ZOOM_STOPS,
  ZOOM_STORAGE_KEY,
  clampZoom,
  formatZoom,
  isDefaultZoom,
  nextZoom,
  parseZoom,
  rootFontSize,
  zoomKeyCommand,
  zoomPercent,
  zoomWheelCommand,
} from "../logic/zoom.ts";

// -------------------------------------------------------------------- stops

test("the stops span the advertised range and include 100%", () => {
  assert.equal(ZOOM_STOPS[0], ZOOM_MIN);
  assert.equal(ZOOM_STOPS[ZOOM_STOPS.length - 1], ZOOM_MAX);
  assert.ok(ZOOM_STOPS.includes(ZOOM_DEFAULT), "100% must be reachable by stepping");
  assert.ok(ZOOM_MIN >= 0.5 && ZOOM_MAX <= 3, "a sane range, not a novelty");
});

test("the stops only ever increase", () => {
  for (let i = 1; i < ZOOM_STOPS.length; i++) {
    assert.ok(ZOOM_STOPS[i] > ZOOM_STOPS[i - 1], `stop ${i} is not larger`);
  }
});

// ------------------------------------------------------------------ stepping

test("a step moves exactly one stop", () => {
  assert.equal(nextZoom(1, "in"), 1.1);
  assert.equal(nextZoom(1, "out"), 0.9);
  assert.equal(nextZoom(1.1, "in"), 1.25);
  assert.equal(nextZoom(0.9, "out"), 0.8);
});

test("stepping is clamped at both ends and never wraps", () => {
  assert.equal(nextZoom(ZOOM_MAX, "in"), ZOOM_MAX);
  assert.equal(nextZoom(ZOOM_MIN, "out"), ZOOM_MIN);
  // Holding the key down must settle, not oscillate.
  let z = 1;
  for (let i = 0; i < 50; i++) z = nextZoom(z, "in");
  assert.equal(z, ZOOM_MAX);
  for (let i = 0; i < 50; i++) z = nextZoom(z, "out");
  assert.equal(z, ZOOM_MIN);
});

test("a value between stops moves to the next stop past it, in either direction", () => {
  assert.equal(nextZoom(1.05, "in"), 1.1);
  assert.equal(nextZoom(1.05, "out"), 1);
  assert.equal(nextZoom(1.37, "in"), 1.5);
  assert.equal(nextZoom(1.37, "out"), 1.25);
});

test("reset is one stop, from anywhere", () => {
  assert.equal(nextZoom(ZOOM_MAX, "reset"), ZOOM_DEFAULT);
  assert.equal(nextZoom(ZOOM_MIN, "reset"), ZOOM_DEFAULT);
  assert.equal(nextZoom(1.37, "reset"), ZOOM_DEFAULT);
});

test("a step out of an out-of-range value comes back into range", () => {
  assert.equal(nextZoom(5, "out"), 1.75);
  assert.equal(nextZoom(0.1, "in"), 0.8);
});

// ------------------------------------------------------------------ clamping

test("clamping holds the range and survives nonsense", () => {
  assert.equal(clampZoom(0.1), ZOOM_MIN);
  assert.equal(clampZoom(9), ZOOM_MAX);
  assert.equal(clampZoom(1.25), 1.25);
  assert.equal(clampZoom(NaN), ZOOM_DEFAULT);
  assert.equal(clampZoom(Infinity), ZOOM_MAX);
});

test("the root font size is the base times the zoom, and nothing else", () => {
  assert.equal(rootFontSize(1), ZOOM_BASE_PX);
  assert.equal(rootFontSize(2), ZOOM_BASE_PX * 2);
  assert.equal(rootFontSize(0.7), ZOOM_BASE_PX * 0.7);
  // Out of range in means clamped out, so no zoom can produce an unreadable
  // or absurd root size.
  assert.equal(rootFontSize(50), ZOOM_BASE_PX * ZOOM_MAX);
});

test("the readout is a whole percent", () => {
  assert.equal(zoomPercent(1), 100);
  assert.equal(zoomPercent(1.25), 125);
  assert.equal(zoomPercent(0.7), 70);
  assert.equal(formatZoom(1.75), "175%");
  assert.equal(isDefaultZoom(1), true);
  assert.equal(isDefaultZoom(1.1), false);
});

// ------------------------------------------------------------- stored value

test("a stored zoom round-trips through a string", () => {
  for (const stop of ZOOM_STOPS) {
    assert.equal(parseZoom(String(stop)), stop);
  }
});

test("a missing or corrupt stored zoom is 100%, never a broken interface", () => {
  assert.equal(parseZoom(null), ZOOM_DEFAULT);
  assert.equal(parseZoom(undefined), ZOOM_DEFAULT);
  assert.equal(parseZoom(""), ZOOM_DEFAULT);
  assert.equal(parseZoom("banana"), ZOOM_DEFAULT);
  assert.equal(parseZoom("0"), ZOOM_DEFAULT);
  assert.equal(parseZoom("-2"), ZOOM_DEFAULT);
  // A value from a future build with a wider range is clamped, not honoured.
  assert.equal(parseZoom("12"), ZOOM_MAX);
});

// ---------------------------------------------------------------- the keys

const key = (k, over = {}) => ({ key: k, ctrlKey: true, ...over });

test("Ctrl+= zooms in, because that is what an unshifted + reports", () => {
  assert.equal(zoomKeyCommand(key("=", { code: "Equal" })), "in");
  assert.equal(zoomKeyCommand(key("+", { code: "Equal", shiftKey: true })), "in");
});

test("the numpad works too, by key or by code", () => {
  assert.equal(zoomKeyCommand(key("+", { code: "NumpadAdd" })), "in");
  assert.equal(zoomKeyCommand(key("Add", { code: "NumpadAdd" })), "in");
  assert.equal(zoomKeyCommand(key("-", { code: "NumpadSubtract" })), "out");
  assert.equal(zoomKeyCommand(key("Subtract", { code: "NumpadSubtract" })), "out");
  // NumLock off: the key reports as Insert and only the code is left.
  assert.equal(zoomKeyCommand(key("Insert", { code: "Numpad0" })), "reset");
  assert.equal(zoomKeyCommand(key("0", { code: "Numpad0" })), "reset");
});

test("Ctrl+- zooms out and Ctrl+0 resets", () => {
  assert.equal(zoomKeyCommand(key("-", { code: "Minus" })), "out");
  assert.equal(zoomKeyCommand(key("_", { code: "Minus", shiftKey: true })), "out");
  assert.equal(zoomKeyCommand(key("0", { code: "Digit0" })), "reset");
});

test("Cmd counts as Ctrl", () => {
  assert.equal(zoomKeyCommand({ key: "=", metaKey: true }), "in");
  assert.equal(zoomKeyCommand({ key: "0", metaKey: true }), "reset");
});

test("without a modifier nothing is a zoom — typing a minus is typing a minus", () => {
  assert.equal(zoomKeyCommand({ key: "-" }), null);
  assert.equal(zoomKeyCommand({ key: "0", code: "Digit0" }), null);
  assert.equal(zoomKeyCommand({ key: "=", ctrlKey: false }), null);
});

test("Alt disqualifies: Ctrl+Alt+key belongs to the layout, not to us", () => {
  assert.equal(zoomKeyCommand(key("-", { altKey: true })), null);
  assert.equal(zoomKeyCommand(key("0", { altKey: true })), null);
});

test("the app's other Ctrl shortcuts are left alone", () => {
  assert.equal(zoomKeyCommand(key("p")), null);
  assert.equal(zoomKeyCommand(key("P")), null);
  assert.equal(zoomKeyCommand(key("1", { code: "Digit1" })), null);
  assert.equal(zoomKeyCommand(key("Escape")), null);
  assert.equal(zoomKeyCommand(key("ArrowLeft")), null);
});

// --------------------------------------------------------------- the wheel

test("Ctrl+wheel zooms by direction only", () => {
  assert.equal(zoomWheelCommand({ deltaY: -1, ctrlKey: true }), "in");
  assert.equal(zoomWheelCommand({ deltaY: -240, ctrlKey: true }), "in");
  assert.equal(zoomWheelCommand({ deltaY: 3, ctrlKey: true }), "out");
  assert.equal(zoomWheelCommand({ deltaY: 0, ctrlKey: true }), null);
});

test("a bare wheel is not a zoom — over the video it is the volume", () => {
  assert.equal(zoomWheelCommand({ deltaY: -120 }), null);
  assert.equal(zoomWheelCommand({ deltaY: 120, ctrlKey: false }), null);
});

// public/boot.js applies the remembered zoom before the bundle exists to be
// imported from. That duplicates the storage key and the bounds, and a silent
// drift there would make the app open at the wrong size with nothing to show
// for it.
test("the zoom bootstrap agrees with the module", async () => {
  const { readFileSync } = await import("node:fs");
  const html = readFileSync(new URL("../../public/boot.js", import.meta.url), "utf8");
  const css = readFileSync(new URL("../../public/boot.css", import.meta.url), "utf8");
  assert.match(css, /html\.zoom-pending\s*\{\s*visibility:\s*hidden;/);
  assert.ok(
    html.includes(`localStorage.getItem("${ZOOM_STORAGE_KEY}")`),
    "the bootstrap reads a different storage key than the module writes",
  );
  assert.ok(
    html.includes(`Math.min(${ZOOM_MAX}, Math.max(${ZOOM_MIN},`),
    `the bootstrap clamps to different bounds than ${ZOOM_MIN}..${ZOOM_MAX}`,
  );
  // Hidden until zoom settles, and revealed again no matter what happens.
  assert.ok(html.includes("zoom-pending"), "the bootstrap does not hide the page");
  assert.ok(
    html.includes('classList.remove("zoom-pending")'),
    "nothing reveals the page if the bundle never loads",
  );
});

// The recording service serves this page to browsers under a policy that runs
// no inline script and applies no inline style (crates/zapper-server/src/recording/web.rs),
// so index.html loads the bootstrap from files, before the bundle.
test("index.html has no inline script or style", async () => {
  const { readFileSync } = await import("node:fs");
  const html = readFileSync(new URL("../../index.html", import.meta.url), "utf8");
  assert.doesNotMatch(html, /<script(?![^>]*\bsrc=)[^>]*>/, "an inline <script>");
  assert.doesNotMatch(html, /<style/, "an inline <style>");
  assert.doesNotMatch(html, /\son[a-z]+=/i, "an inline event handler");
  const boot = html.indexOf('src="/boot.js"');
  assert.ok(boot > 0 && boot < html.indexOf("/src/main.tsx"), "boot.js runs before the bundle");
  assert.ok(html.indexOf('href="/boot.css"') < boot, "boot.css applies before boot.js hides the page");
});
