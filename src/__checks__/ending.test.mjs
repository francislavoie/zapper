// The words on a picture that stopped by itself.
//
// "I get kicked off and it doesn't tell me what happened" was the report:
// a provider ended the stream for another device and the picture simply
// vanished. These pin that every way a stream can stop says something, and
// that the two that are about the connection limit say so.

import test from "node:test";
import assert from "node:assert/strict";
import { endedMessage } from "../logic/ending.ts";

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
  ]) {
    const m = endedMessage(failed({ kind, status: null, limit: false }));
    assert.ok(m, kind);
    assert.ok(m.title.length > 0, kind);
    assert.ok(m.detail && m.detail.length > 0, kind);
  }
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
