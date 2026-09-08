// The two questions src/audit.ts asks of the live DOM: is this surface light,
// and does this element paint anything at all? Both are colour arithmetic, and
// both are the difference between "it looks fine" and knowing.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  isLightSurface,
  paintsAnything,
  parseCssColor,
  relativeLuminance,
} from "../logic/contrast.ts";

test("parses the forms getComputedStyle actually returns", () => {
  assert.deepEqual(parseCssColor("rgb(13, 15, 18)"), { r: 13, g: 15, b: 18, a: 1 });
  assert.deepEqual(parseCssColor("rgba(0, 0, 0, 0)"), { r: 0, g: 0, b: 0, a: 0 });
  assert.deepEqual(parseCssColor("rgba(255, 255, 255, 0.5)"), {
    r: 255,
    g: 255,
    b: 255,
    a: 0.5,
  });
  assert.deepEqual(parseCssColor("#fff"), { r: 255, g: 255, b: 255, a: 1 });
  assert.deepEqual(parseCssColor("#0d0f12"), { r: 13, g: 15, b: 18, a: 1 });
  assert.deepEqual(parseCssColor("transparent"), { r: 0, g: 0, b: 0, a: 0 });
});

test("an unparseable colour is null, never a guess", () => {
  assert.equal(parseCssColor("color(display-p3 1 1 1)"), null);
  assert.equal(parseCssColor("nonsense"), null);
});

test("luminance is pinned at both ends and ordered in between", () => {
  assert.equal(relativeLuminance({ r: 0, g: 0, b: 0 }), 0);
  assert.equal(relativeLuminance({ r: 255, g: 255, b: 255 }), 1);
  assert.ok(
    relativeLuminance({ r: 13, g: 15, b: 18 }) < relativeLuminance({ r: 38, g: 43, b: 52 }),
  );
});

test("the app's own tokens are all dark surfaces", () => {
  for (const c of ["#0d0f12", "#14171c", "#1b1f26", "#262b34"]) {
    assert.equal(isLightSurface(c), false, `${c} should read as dark`);
  }
});

test("a white panel is caught, whatever notation it arrives in", () => {
  assert.equal(isLightSurface("#ffffff"), true);
  assert.equal(isLightSurface("rgb(255, 255, 255)"), true);
  assert.equal(isLightSurface("rgb(240, 240, 240)"), true);
});

test("a translucent wash is not a surface", () => {
  // The chrome gradients and the scrubber's track are painted over video on
  // purpose; only something solid counts as a panel.
  assert.equal(isLightSurface("rgba(255, 255, 255, 0.14)"), false);
  assert.equal(isLightSurface("rgba(255, 255, 255, 0.6)"), true);
});

test("over the video, any paint at all counts", () => {
  assert.equal(paintsAnything("rgba(0, 0, 0, 0)"), false);
  assert.equal(paintsAnything("transparent"), false);
  assert.equal(paintsAnything("rgba(6, 8, 11, 0.04)"), true);
  assert.equal(paintsAnything("#0d0f12"), true);
  // Unparseable is treated as painting: over the video plane, an unknown is a
  // failure rather than a pass.
  assert.equal(paintsAnything("color(display-p3 0 0 0)"), true);
});
