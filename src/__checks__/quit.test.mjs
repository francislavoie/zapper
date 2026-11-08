// What the page does when the app quits (src/quit.ts).
//
// The desktop app ends the page's process as soon as `window.__zapperQuit`
// resolves, so the goodbye must reach every debounced save (settings,
// recording settings), and must not resolve before the saves it set off have
// been answered: a save still on its way when the process ends is lost.

import test from "node:test";
import assert from "node:assert/strict";

// The page's globals, as far as quit.ts uses them.
globalThis.window = new EventTarget();
globalThis.PageTransitionEvent = class extends Event {};

const { installQuitHook, onPageHide, trackSave } = await import("../quit.ts");
installQuitHook();

test("the goodbye flushes a debounced save and waits for its answer", async () => {
  let answer;
  let answered = false;
  // A pane with a change waiting: its flush sends it, tracked.
  const off = onPageHide(() => {
    void trackSave(
      new Promise((resolve) => {
        answer = resolve;
      }).then(() => {
        answered = true;
      }),
    );
  });
  let quitDone = false;
  const quit = window.__zapperQuit().then(() => {
    quitDone = true;
  });
  // The flush ran inside the goodbye, but its answer has not come.
  assert.equal(typeof answer, "function");
  await new Promise((r) => setTimeout(r, 10));
  assert.equal(quitDone, false, "the goodbye resolved before the save was answered");
  answer();
  await quit;
  assert.equal(answered, true);
  off();
});

test("a failed save does not hold the goodbye up", async () => {
  const off = onPageHide(() => {
    void trackSave(Promise.reject(new Error("the service is down"))).catch(() => {});
  });
  await window.__zapperQuit();
  off();
});

test("an unsubscribed pane is left alone, and nothing pending resolves at once", async () => {
  let ran = false;
  const off = onPageHide(() => {
    ran = true;
  });
  off();
  await window.__zapperQuit();
  assert.equal(ran, false);
});
