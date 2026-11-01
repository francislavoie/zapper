// What the channel list says about a channel it has played: the badge, the
// measured detail behind it and how fresh that is, and the dimmed row of a
// channel that did not load.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  ago,
  failedTip,
  failedToLoad,
  qualityBadge,
  qualityDetail,
} from "../logic/format.ts";

const NOW = 1_800_000_000;

const record = (over = {}) => ({
  codec: "h264",
  width: 1920,
  height: 1080,
  fps: 50,
  interlaced: false,
  isHdr: false,
  audioCodec: "aac",
  audioChannels: 2,
  measuredAt: NOW - 3 * 3600,
  okCount: 4,
  failCount: 0,
  failedAt: null,
  failReason: null,
  ...over,
});

test("ago is a distance, coarsening as it grows", () => {
  assert.equal(ago(NOW - 20, NOW), "just now");
  assert.equal(ago(NOW - 12 * 60, NOW), "12m ago");
  assert.equal(ago(NOW - 3 * 3600, NOW), "3h ago");
  assert.equal(ago(NOW - 30 * 3600, NOW), "yesterday");
  assert.equal(ago(NOW - 5 * 86400, NOW), "5 days ago");
  assert.equal(ago(NOW - 70 * 86400, NOW), "2 months ago");
  assert.equal(ago(NOW - 400 * 86400, NOW), "over a year ago");
  // A clock that moved backwards is not the future.
  assert.equal(ago(NOW + 50, NOW), "just now");
});

test("the badge says fields when the picture is interlaced", () => {
  assert.equal(qualityBadge(record()), "1080p");
  assert.equal(qualityBadge(record({ interlaced: true })), "1080i");
  assert.equal(qualityBadge(record({ height: 576, interlaced: true })), "576i");
  assert.equal(qualityBadge(record({ height: 2160, interlaced: true })), "4K");
  assert.equal(qualityBadge(record({ interlaced: null })), "1080p");
});

test("the detail names everything measured, and when", () => {
  assert.equal(
    qualityDetail(record({ interlaced: true, audioCodec: "eac3", audioChannels: 6 }), NOW),
    "1920×1080 · 50 fps · H.264 · interlaced · E-AC-3 5.1 · checked 3h ago",
  );
  assert.equal(
    qualityDetail(record({ fps: 59.94, codec: "mpeg2video", audioCodec: null, audioChannels: 2 }), NOW),
    "1920×1080 · 59.94 fps · MPEG-2 · stereo · checked 3h ago",
  );
  // A codec nobody has named here is shown as mpv says it.
  assert.match(qualityDetail(record({ codec: "prores" }), NOW), /prores/);
});

test("nothing measured is no detail, whatever else is on record", () => {
  assert.equal(qualityDetail(null, NOW), "");
  assert.equal(qualityDetail(record({ measuredAt: null, height: null, width: null }), NOW), "");
});

test("a channel that failed is dimmed, with when and why", () => {
  const failed = record({ failedAt: NOW - 2 * 3600, failReason: "refused by the provider (HTTP 403)" });
  assert.equal(failedToLoad(failed), true);
  assert.equal(failedTip(failed, NOW), "Didn't load 2h ago — refused by the provider (HTTP 403)");
  assert.equal(failedTip(record({ failedAt: NOW - 60 }), NOW), "Didn't load 1m ago");
});

test("a channel never tried, or that played since, is not dimmed", () => {
  assert.equal(failedToLoad(null), false);
  assert.equal(failedToLoad(record()), false);
  assert.equal(failedTip(record(), NOW), null);
});
