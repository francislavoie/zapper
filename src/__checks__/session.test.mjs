// What the app remembers between launches.
//
// Every value here is read back from storage that can be hand-edited, can
// have been written by an older build with a different shape, and can be
// truncated by a browser reclaiming space. The rule is that a bad value puts
// the app at the top of the list — never blank, never thrown.

import test from "node:test";
import assert from "node:assert/strict";
import {
  EMPTY_SESSION,
  parseSession,
  serialiseSession,
} from "../logic/session.ts";

test("a missing or empty session is the empty one", () => {
  assert.deepEqual(parseSession(null), EMPTY_SESSION);
  assert.deepEqual(parseSession(""), EMPTY_SESSION);
});

test("a round trip preserves everything", () => {
  const s = {
    categoryId: "sports-uk",
    railTab: "favourites",
    railScrollTop: 420,
    channelScrollTop: 13_000,
    lastStreamId: 107037,
    previousStreamId: 107040,
    compressor: true,
    railCollapsed: true,
    volume: 135,
    muted: true,
  };
  assert.deepEqual(parseSession(serialiseSession(s)), s);
});

test("the volume the app was closed at comes back, within the slider's range", () => {
  assert.equal(parseSession('{"volume":72}').volume, 72);
  // Never set: mpv's default stands, so there is nothing to restore.
  assert.equal(parseSession("{}").volume, null);
  assert.equal(parseSession('{"volume":"loud"}').volume, null);
  // A hand-edited or corrupt value is clamped, not trusted.
  assert.equal(parseSession('{"volume":900}').volume, 200);
  assert.equal(parseSession('{"volume":-5}').volume, 0);
  assert.equal(parseSession('{"muted":true}').muted, true);
  assert.equal(parseSession('{"muted":1}').muted, false);
});

test("a folded rail is remembered, and only a real true folds it", () => {
  assert.equal(parseSession('{"railCollapsed":true}').railCollapsed, true);
  assert.equal(parseSession("{}").railCollapsed, false);
  assert.equal(parseSession('{"railCollapsed":"yes"}').railCollapsed, false);
});

test("rubbish in storage starts fresh rather than throwing", () => {
  // Truncated by a quota sweep, which is the realistic way this breaks.
  assert.deepEqual(parseSession('{"categoryId":"spo'), EMPTY_SESSION);
  assert.deepEqual(parseSession("not json at all"), EMPTY_SESSION);
  // Valid JSON of the wrong shape, which is what an older build leaves.
  assert.deepEqual(parseSession("null"), EMPTY_SESSION);
  assert.deepEqual(parseSession("[1,2,3]"), EMPTY_SESSION);
  assert.deepEqual(parseSession('"a string"'), EMPTY_SESSION);
});

test("nonsense offsets land at the top, not off the end", () => {
  const at = (raw) => parseSession(raw);
  assert.equal(at('{"railScrollTop":-10}').railScrollTop, 0);
  assert.equal(at('{"railScrollTop":"600"}').railScrollTop, 0);
  assert.equal(at('{"channelScrollTop":null}').channelScrollTop, 0);
  // Infinity and NaN do not survive JSON, but a hand-edited file can carry
  // anything, and 1e400 parses as Infinity.
  assert.equal(at('{"channelScrollTop":1e400}').channelScrollTop, 0);
  // A fractional offset is floored rather than rejected: it is a real
  // position from a fractional device pixel ratio.
  assert.equal(at('{"channelScrollTop":40.7}').channelScrollTop, 40);
});

test("an unknown tab falls back to categories", () => {
  assert.equal(parseSession('{"railTab":"favourites"}').railTab, "favourites");
  assert.equal(parseSession('{"railTab":"whatever"}').railTab, "categories");
  assert.equal(parseSession("{}").railTab, "categories");
});

test("a channel id has to be a whole number to be remembered", () => {
  assert.equal(parseSession('{"lastStreamId":12}').lastStreamId, 12);
  assert.equal(parseSession('{"lastStreamId":"12"}').lastStreamId, null);
  assert.equal(parseSession('{"lastStreamId":1.5}').lastStreamId, null);
  // Zero is a legitimate id and must not be lost to a falsiness check.
  assert.equal(parseSession('{"lastStreamId":0}').lastStreamId, 0);
});

test("an empty category id is no category, not a category called nothing", () => {
  assert.equal(parseSession('{"categoryId":""}').categoryId, null);
  assert.equal(parseSession('{"categoryId":"uk"}').categoryId, "uk");
});

test("the compressor is remembered, and only a real true counts", () => {
  assert.equal(parseSession('{"compressor":true}').compressor, true);
  assert.equal(parseSession('{"compressor":false}').compressor, false);
  assert.equal(parseSession("{}").compressor, false);
  // Truthy rubbish from an older build must not silently turn it on: the
  // failure mode is audio that has been quietly processed for weeks.
  assert.equal(parseSession('{"compressor":1}').compressor, false);
  assert.equal(parseSession('{"compressor":"yes"}').compressor, false);
});

test("P's channel comes back beside the channel, and junk does not", () => {
  assert.equal(parseSession('{"previousStreamId":42}').previousStreamId, 42);
  assert.equal(parseSession("{}").previousStreamId, null);
  assert.equal(parseSession('{"previousStreamId":"42"}').previousStreamId, null);
  assert.equal(parseSession('{"previousStreamId":4.2}').previousStreamId, null);
});
