// The shared transport: what the docked picture's controls do that the
// player's do too, and the few places they are allowed to differ.
//
// The docked controls once lacked the scrubber, the scrollback readout and
// the double-click zones, because they were a second copy of the bar that
// had stopped being kept up. Both now render TransportBar; what is checked
// here is the logic either side of it — the click that has to wait for a
// possible double-click, what each zone does, and the captions in fewer
// words — plus a guard that the second copy does not grow back.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  DOUBLE_CLICK_MS,
  SKIP_SECONDS,
  clickIntent,
  slotAction,
  zoneAction,
} from "../logic/gestures.ts";
import { scrubLabels } from "../logic/format.ts";
import { hintFitsAbove } from "../logic/scrubber.ts";

// ------------------------------------------------------------- double-click

test("the middle zone is fullscreen and the sides seek by the skip step", () => {
  assert.deepEqual(zoneAction("middle", false), { kind: "fullscreen" });
  assert.deepEqual(zoneAction("middle", true), { kind: "fullscreen" });
  assert.deepEqual(zoneAction("back", true), { kind: "seek", zone: "back", by: -SKIP_SECONDS });
  assert.deepEqual(zoneAction("forward", false), {
    kind: "seek",
    zone: "forward",
    by: SKIP_SECONDS,
  });
});

test("forward at the live edge does nothing, rather than promise a jump", () => {
  assert.equal(zoneAction("forward", true), null);
});

test("a click is single, the second of a pair is double, a third is nothing", () => {
  assert.equal(clickIntent(1), "single");
  // Synthesised clicks carry no count; they are still a click.
  assert.equal(clickIntent(0), "single");
  assert.equal(clickIntent(2), "double");
  assert.equal(clickIntent(3), "ignore");
  assert.equal(clickIntent(Number.NaN), "single");
});

test("the wait for a second click is short enough not to feel like lag", () => {
  // Every single click on the docked picture pays it. Long enough for an
  // ordinary double-click, and well under the half second where a click
  // starts to feel ignored.
  assert.ok(DOUBLE_CLICK_MS >= 200 && DOUBLE_CLICK_MS <= 350, `${DOUBLE_CLICK_MS}ms`);
});

// ------------------------------------------------------------- single click

test("a click on the docked picture resumes when paused, else opens the player", () => {
  assert.equal(slotAction("paused", false), "resume");
  assert.equal(slotAction("playing", false), "watch");
  assert.equal(slotAction("opening", false), "watch");
  assert.equal(slotAction("failed", false), "watch");
});

test("with nothing playing it starts the restored channel, or does nothing", () => {
  assert.equal(slotAction("stopped", true), "start");
  assert.equal(slotAction("stopped", false), null);
});

// ---------------------------------------------------------------- captions

const base = { archiveDays: 3, bufferedBack: 672, hasChannel: true };

test("the bar's captions are unchanged", () => {
  assert.deepEqual(scrubLabels({ ...base, hasArchive: true, compact: false }), [
    "3-day archive · seek reconnects",
    "buffer 11m12s · instant",
  ]);
  assert.deepEqual(scrubLabels({ ...base, hasArchive: false, compact: false }), [
    "local buffer only — no catchup on this channel",
    "11m12s of scrollback",
  ]);
  assert.deepEqual(
    scrubLabels({ ...base, hasArchive: false, hasChannel: false, compact: false }),
    ["local buffer only", "11m12s of scrollback"],
  );
});

test("the docked captions say the same two facts, shorter", () => {
  const archived = scrubLabels({ ...base, hasArchive: true, compact: true });
  const local = scrubLabels({ ...base, hasArchive: false, compact: true });
  assert.deepEqual(archived, ["3d archive", "11m12s buffer"]);
  assert.deepEqual(local, ["no catchup", "11m12s back"]);
  // Short enough for a 14rem picture at 9px: well under half its width each.
  for (const s of [...archived, ...local]) assert.ok(s.length <= 14, s);
});

// ------------------------------------------------------------ preview card

test("the preview opens above the bar while it fits inside the picture", () => {
  // The default docked picture: bar 138px below the picture's top, card 78.
  assert.equal(hintFitsAbove(176, 78, 38), true);
  // The smallest: bar 37px below the top. Above, the card would be cut off
  // by the window's edge, so it opens below.
  assert.equal(hintFitsAbove(75, 78, 38), false);
  // Exactly flush is still inside.
  assert.equal(hintFitsAbove(116, 78, 38), true);
  // Nothing measured: the bar's ordinary placement.
  assert.equal(hintFitsAbove(Number.NaN, 78, 38), true);
});

// ------------------------------------------------------------ one copy only

test("the docked controls draw the player's TransportBar, not their own", () => {
  const read = (f) => readFileSync(new URL(`../components/${f}`, import.meta.url), "utf8");
  const mini = read("MiniControls.tsx");
  const chrome = read("PlayerChrome.tsx");
  for (const [name, src] of [
    ["MiniControls", mini],
    ["PlayerChrome", chrome],
  ]) {
    assert.match(src, /<TransportBar\b/, `${name} should render TransportBar`);
    // The buttons live in TransportBar. A transport button defined here
    // again is the second copy coming back.
    for (const call of ["togglePause", "seekRelative", "seekLive", "setCompressor", "<Scrubber"]) {
      if (name === "PlayerChrome" && call === "togglePause") continue; // click-to-resume
      assert.ok(!src.includes(call), `${name} defines its own ${call} again`);
    }
  }
  assert.match(mini, /<TransportBar\s+compact\b/);
});
