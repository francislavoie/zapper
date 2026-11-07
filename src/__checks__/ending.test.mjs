// The words on a picture that stopped by itself.
//
// "I get kicked off and it doesn't tell me what happened" was the report:
// a provider ended the stream for another device and the picture simply
// vanished. These pin that every way a stream can stop says something, and
// that the two that are about the connection limit say so.

import test from "node:test";
import assert from "node:assert/strict";
import { endedActions, endedMessage, primaryLabel, reconnectMessage, startingLine } from "../logic/ending.ts";

const failed = (ending, error = "the stream ended") => ({ state: "failed", ending, error });

test("nothing to say while a stream is fine, or was stopped on purpose", () => {
  for (const state of ["stopped", "opening", "playing", "paused"]) {
    assert.equal(endedMessage({ state, ending: null, error: null }), null, state);
  }
});

test("a stream taken away mid-play says the provider ended it", () => {
  const m = endedMessage(failed({ kind: "dropped", status: null, limit: false }));
  assert.equal(m.title, "Playback stopped — your provider ended this stream.");
  assert.equal(m.detail, "Another device may be using your connection.");
  assert.equal(m.code, null);
});

test("a refusal asks about the limit, and states it when the server did", () => {
  const asked = endedMessage(failed({ kind: "refused", status: 403, limit: false }));
  assert.equal(asked.title, "Your provider refused the stream (too many connections?)");
  assert.equal(asked.code, "HTTP 403");

  const said = endedMessage(failed({ kind: "refused", status: 509, limit: true }));
  assert.match(said.title, /too many connections\.$/);
  assert.doesNotMatch(said.title, /\?/);
  assert.equal(said.code, "HTTP 509");
});

test("every kind of ending has a headline and something to go on", () => {
  for (const kind of [
    "dropped",
    "refused",
    "notVideo",
    "unavailable",
    "serverError",
    "network",
    "noVideo",
    "other",
    "beyondArchive",
  ]) {
    const m = endedMessage(failed({ kind, status: null, limit: false }));
    assert.ok(m, kind);
    assert.ok(m.title.length > 0, kind);
    assert.ok(m.detail && m.detail.length > 0, kind);
  }
});

test("a catch-up older than the archive keeps says so, plainly", () => {
  const m = endedMessage(failed({ kind: "beyondArchive", status: 404, limit: false }));
  assert.equal(m.title, "That's further back than your provider keeps for this channel.");
  assert.equal(m.code, "HTTP 404");
  // Not the connection limit, and nothing about another device.
  assert.doesNotMatch(m.title + m.detail, /connection|device/i);
  // Tuning again plays the channel live, and the button says so.
  const a = endedActions(failed({ kind: "beyondArchive", status: 404, limit: false }));
  assert.equal(a.primary, "watchLive");
  assert.equal(a.note, null);
  assert.equal(a.dismiss, true);
  assert.equal(primaryLabel("watchLive"), "Watch live");
  assert.equal(primaryLabel("watchHere"), "Watch here anyway");
  assert.equal(primaryLabel("retry"), "Retry");
});

test("the network is not blamed on the provider", () => {
  const m = endedMessage(failed({ kind: "network", status: null, limit: false }));
  assert.doesNotMatch(m.title + m.detail, /provider/i);
});

test("a failure with no ending shows the backend's own words", () => {
  // An HDHomeRun's busy tuners, say: the source's explanation beats a guess.
  const why = "All 2 tuners on the HDHomeRun are in use.";
  const m = endedMessage({ state: "failed", ending: null, error: why });
  assert.equal(m.title, "Couldn't play this channel.");
  assert.equal(m.detail, why);
});

// ------------------------------------------------------------- on the way in

test("a channel on its way says whether it is opening or joining live", () => {
  assert.equal(startingLine({ state: "opening", joiningLive: false }), "Opening…");
  // The first frame is up and held while the connect burst arrives.
  assert.equal(startingLine({ state: "opening", joiningLive: true }), "Joining live…");
  for (const state of ["stopped", "playing", "paused", "failed"]) {
    assert.equal(startingLine({ state, joiningLive: false }), null, state);
  }
  // Reconnecting, waiting or attempting, says so rather than "Opening…".
  const reconnect = { attempt: 3, of: 4, checking: false };
  assert.equal(startingLine({ state: "failed", joiningLive: false, reconnect }), "Reconnecting… (3 of 4)");
  assert.equal(startingLine({ state: "opening", joiningLive: false, reconnect }), "Reconnecting… (3 of 4)");
});

// ------------------------------------------------------------- what to press

test("where another device is the likely reason, the button says what it does", () => {
  const at = (kind, limit = false) => ({
    state: "failed",
    ending: { kind, status: null, limit, slots: null },
    reconnect: null,
  });
  for (const kind of ["dropped", "notVideo", "refused", "inUse"]) {
    const a = endedActions(at(kind));
    assert.equal(a.primary, "watchHere", kind);
    assert.ok(a.note && /other device|another device/.test(a.note), kind);
    assert.equal(a.dismiss, true);
  }
  assert.equal(endedActions(at("refused", true)).note, "This will stop playback on the other device.");
  assert.equal(endedActions(at("inUse")).note, "This will stop playback on the other device.");
  // Nobody else involved: a plain Retry, with nothing to warn about.
  for (const kind of ["network", "unavailable", "serverError", "noVideo", "other"]) {
    const a = endedActions(at(kind));
    assert.equal(a.primary, "retry", kind);
    assert.equal(a.note, null, kind);
  }
  assert.equal(endedActions({ state: "failed", ending: null, reconnect: null }).primary, "retry");
  assert.equal(endedActions({ state: "playing", ending: null, reconnect: null }), null);
});

test("a reconnect in progress can only be cancelled", () => {
  const p = {
    state: "failed",
    error: "x",
    ending: { kind: "network", status: null, limit: false, slots: null },
    reconnect: { attempt: 2, of: 4, checking: false },
  };
  assert.deepEqual(endedActions(p), { primary: "cancel", note: null, dismiss: false });
  // Opening the attempt itself is still a reconnect, and still cancellable.
  assert.equal(endedActions({ ...p, state: "opening" }).primary, "cancel");
  const m = reconnectMessage(p);
  assert.equal(m.title, "Reconnecting… (2 of 4)");
  assert.equal(m.detail, "Couldn't reach the server.", "the reason stays under it");
  assert.match(reconnectMessage({ ...p, reconnect: { ...p.reconnect, checking: true } }).detail, /connection free/);
  assert.equal(reconnectMessage({ ...p, reconnect: null }), null);
});

test("an account in use elsewhere says how many connections", () => {
  const m = endedMessage({
    state: "failed",
    error: "x",
    ending: { kind: "inUse", status: null, limit: true, slots: { max: 1, active: 1 } },
  });
  assert.equal(m.title, "Your account is in use on another device (1 of 1 connection).");
  const two = endedMessage({
    state: "failed",
    error: "x",
    ending: { kind: "inUse", status: null, limit: true, slots: { max: 2, active: 2 } },
  });
  assert.match(two.title, /\(2 of 2 connections\)/);
});
