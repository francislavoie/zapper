// "Try another source:" under an off-air channel: which channels with the
// same guide id are offered, in what order, and how each reads. The backend
// answers the same rule from the catalogue (store::alternatives_for); this
// is the mock's copy of it, and the row's words.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import { ALTERNATIVES_MAX, alternativeLine, rankAlternatives } from "../logic/alternatives.ts";

const quality = (over = {}) => ({
  codec: "h264",
  width: 1280,
  height: 720,
  fps: 50,
  interlaced: false,
  isHdr: false,
  audioCodec: null,
  audioChannels: null,
  measuredAt: null,
  okCount: 0,
  failCount: 0,
  failedAt: null,
  failReason: null,
  ...over,
});

const chan = (streamId, num, q = null, over = {}) => ({
  streamId,
  name: `XX: Channel ${streamId}`,
  cleanName: `Channel ${streamId}`,
  num,
  logoUrl: null,
  categoryId: "1",
  hasArchive: false,
  archiveDays: null,
  isSeparator: false,
  quality: q,
  nowPlaying: null,
  ...over,
});

const ids = (list) => list.map((c) => c.streamId);

test("those that played come first, newest measurement leading, then by number", () => {
  const group = [
    chan(1, 1),
    chan(2, 40, quality({ okCount: 3, measuredAt: 100 })),
    chan(3, 30, quality({ okCount: 1, measuredAt: 200 })),
    chan(4, 20),
    chan(5, 10),
    chan(6, null),
  ];
  assert.deepEqual(ids(rankAlternatives(group, 1)), [3, 2, 5]);
  assert.deepEqual(ids(rankAlternatives(group, 1, 10)), [3, 2, 5, 4, 6], "unnumbered last");
});

test("itself, dividers, failures and off-air channels are never offered", () => {
  const group = [
    chan(1, 1),
    chan(2, 2, quality({ okCount: 5, measuredAt: 900, failedAt: 950, failReason: "off air" })),
    chan(3, 3, quality({ failedAt: 950, failReason: "refused by the provider (HTTP 403)" })),
    chan(4, 4, null, { isSeparator: true }),
    chan(5, 5),
  ];
  assert.deepEqual(ids(rankAlternatives(group, 1)), [5]);
  assert.deepEqual(rankAlternatives([chan(1, 1)], 1), [], "alone: nothing to offer");
});

test("at most three", () => {
  const many = Array.from({ length: 9 }, (_, i) => chan(i + 2, i + 2));
  assert.equal(ALTERNATIVES_MAX, 3);
  assert.equal(rankAlternatives(many, 1).length, 3);
});

test("a row reads number, name and the resolution last measured", () => {
  const line = alternativeLine(chan(7, 107, quality({ height: 1080 })));
  assert.deepEqual(line, { num: "107", name: "Channel 7", fullName: "XX: Channel 7", resolution: "1080p" });
  // The list's own label for a subchannel; no resolution when never measured.
  const sub = alternativeLine(chan(8, 5001, null, { numLabel: "5.1" }));
  assert.equal(sub.num, "5.1");
  assert.equal(sub.resolution, null);
  assert.equal(alternativeLine(chan(9, null)).num, "");
});
