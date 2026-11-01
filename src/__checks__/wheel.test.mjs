// Which wheel events the page applies itself, and by how much.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import { stepPixels, takesWheel } from "../logic/wheel.ts";

const ev = (p = {}) => ({
  deltaX: 0,
  deltaY: 53,
  deltaMode: 0,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  defaultPrevented: false,
  ...p,
});

test("vertical wheel events are taken, whole notches and fine steps alike", () => {
  assert.equal(takesWheel(ev()), true);
  assert.equal(takesWheel(ev({ deltaY: -53 })), true);
  // A high-resolution or free-spinning wheel's fractional step: indistin-
  // guishable from a touchpad, and it must not be left to glide.
  assert.equal(takesWheel(ev({ deltaY: 6.6 })), true);
});

test("zoom, horizontal, and already-handled wheels are left alone", () => {
  assert.equal(takesWheel(ev({ ctrlKey: true })), false, "Ctrl+wheel is zoom");
  assert.equal(takesWheel(ev({ metaKey: true })), false);
  assert.equal(takesWheel(ev({ shiftKey: true })), false, "Shift+wheel is horizontal");
  assert.equal(takesWheel(ev({ deltaX: 80, deltaY: 10 })), false, "a sideways swipe");
  assert.equal(takesWheel(ev({ defaultPrevented: true })), false, "volume over the picture");
  assert.equal(takesWheel(ev({ deltaY: 0 })), false);
});

test("a fast spin moves the full distance: steps add up exactly", () => {
  // Twenty notches in quick succession is twenty notches of travel. The
  // engine's animation restarted on each and covered a fraction of this.
  let top = 0;
  for (let i = 0; i < 20; i++) top += stepPixels({ deltaY: 53, deltaMode: 0 }, 500);
  assert.equal(top, 20 * 53);
});

test("lines and pages are converted to pixels", () => {
  assert.equal(stepPixels({ deltaY: 3, deltaMode: 1 }, 500), 120);
  assert.equal(stepPixels({ deltaY: -1, deltaMode: 2 }, 500), -500);
});
