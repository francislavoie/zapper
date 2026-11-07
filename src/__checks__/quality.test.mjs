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
  hhmm,
  OFF_AIR_REASON,
  offAir,
  offAirLabel,
  qualityBadge,
  qualityDetail,
  setTimeFormat,
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

test("a channel found off air says so on its row, with when", () => {
  setTimeFormat("24h");
  // Local times, so the day boundary is the machine's own.
  const local = (d, h, m = 0) => new Date(2027, 0, d, h, m).getTime() / 1000;
  const NOW = local(15, 12);
  const at = local(15, 10);
  const off = record({ failedAt: at, failReason: OFF_AIR_REASON, failCount: 1 });
  assert.equal(OFF_AIR_REASON, "off air", "spelled as ending::OFF_AIR_REASON in Rust");
  assert.equal(offAir(off), true);
  assert.equal(failedToLoad(off), true, "dimmed like any failure");
  assert.match(offAirLabel(off, NOW), /^Off air · \d\d:\d\d$/);
  assert.equal(offAirLabel(off, NOW), `Off air · ${hhmm(at)}`);
  assert.equal(
    failedTip(off, NOW),
    `Off air · ${hhmm(at)} — your provider sent a placeholder instead of the channel`,
  );
  // On another day the clock time alone would read as today's: how long
  // ago instead, even when that was only an hour, across midnight.
  const lateLastNight = record({ failedAt: local(14, 23, 30), failReason: OFF_AIR_REASON });
  assert.equal(offAirLabel(lateLastNight, local(15, 0, 30)), "Off air · 1h ago");
  const old = record({ failedAt: local(14, 6), failReason: OFF_AIR_REASON });
  assert.equal(offAirLabel(old, local(15, 12)), "Off air · yesterday");
  // Early the same morning is still today's clock.
  assert.equal(offAirLabel(record({ failedAt: local(15, 0, 5), failReason: OFF_AIR_REASON }), NOW), `Off air · ${hhmm(local(15, 0, 5))}`);
  setTimeFormat("auto");
});

test("only an off-air failure is labelled off air, and a play clears it", () => {
  const refused = record({ failedAt: NOW - 60, failReason: "refused by the provider (HTTP 403)" });
  assert.equal(offAir(refused), false);
  assert.equal(offAirLabel(refused, NOW), null);
  assert.equal(offAirLabel(record(), NOW), null, "the next live play cleared failedAt");
  assert.equal(offAirLabel(record({ failedAt: null, failReason: OFF_AIR_REASON }), NOW), null);
  assert.equal(offAirLabel(null, NOW), null);
});
