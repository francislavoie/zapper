// The lists read the catalogue again after a sync has changed it.
//
// They keep what they fetched — the rail its categories, the list its pages —
// and nothing in a kept page says the mirror under it has been rewritten. The
// sync's status events are the only news of that, so which transitions count
// as news is the whole fix: miss one and a first sync leaves "No categories
// yet" on screen over a catalogue that has arrived, until a restart.

import test from "node:test";
import assert from "node:assert/strict";
import { refreshingTotal, syncChangedCatalogue } from "../logic/catalogueRefresh.ts";

const status = (phase, channelsTotal = 0, lastSynced = null) => ({
  phase,
  message: "",
  channelsTotal,
  lastSynced,
});

/** Every status a pass publishes, in order, as `run_steps` sends them: each
 *  phase once as it starts and once with its result, then idle. */
function pass(before, after, at) {
  return [
    status("categories", before.channelsTotal, before.lastSynced),
    status("categories", before.channelsTotal, before.lastSynced),
    status("channels", before.channelsTotal, before.lastSynced),
    status("channels", after, before.lastSynced),
    status("epg", after, before.lastSynced),
    status("epg", after, before.lastSynced),
    status("idle", after, at),
  ];
}

function refreshes(from, statuses) {
  let prev = from;
  const out = [];
  for (const s of statuses) {
    out.push(syncChangedCatalogue(prev, s));
    prev = s;
  }
  return out;
}

test("a first sync into an empty catalogue is noticed before the pass ends", () => {
  const empty = status("idle", 0, null);
  const seen = refreshes(empty, pass(empty, 60000, 1_700_000_000));
  // The channels are readable once their phase ends; waiting for the guide
  // would leave the list empty for as long as the guide takes.
  const channelsLanded = 4; // the first status after the channels phase
  assert.ok(seen.slice(0, channelsLanded + 1).some(Boolean), `not before the guide: ${seen}`);
  // And again at the end, for what the guide changed.
  assert.equal(seen.at(-1), true);
});

test("each phase that ends is a refresh, and a status within a phase is not", () => {
  const before = status("idle", 500, 1_699_000_000);
  const seen = refreshes(before, pass(before, 500, 1_700_000_000));
  // Starting the pass changes nothing; neither does a phase's own result
  // message while the phase is still the current one.
  assert.deepEqual(seen, [false, false, true, false, true, false, true]);
});

test("a refresh from the provider with the same channel count still refreshes", () => {
  // Renamed channels, a moved category, new programmes: the count says
  // nothing about those.
  const before = status("idle", 500, 1_699_000_000);
  assert.ok(refreshes(before, pass(before, 500, 1_700_000_000)).some(Boolean));
});

test("a failed phase is a refresh too: the phases before it landed", () => {
  const seen = refreshes(status("idle", 0, null), [
    status("categories"),
    status("channels"),
    status("failed"),
  ]);
  assert.equal(seen.at(-1), true);
});

test("a pass whose phases were not seen is caught by what it left behind", () => {
  // Subscribed late, or a status read once rather than pushed.
  assert.equal(syncChangedCatalogue(status("idle", 0, null), status("idle", 120, 1_700_000_000)), true);
  assert.equal(
    syncChangedCatalogue(status("idle", 120, 1_700_000_000), status("idle", 120, 1_700_003_600)),
    true,
  );
});

test("statuses that report nothing new are not a refresh", () => {
  const s = status("idle", 120, 1_700_000_000);
  assert.equal(syncChangedCatalogue(s, { ...s, message: "something else" }), false);
  assert.equal(syncChangedCatalogue(status("failed", 120, 1), status("failed", 120, 1)), false);
  // Leaving `failed` for a new pass is the pass starting, not data landing.
  assert.equal(syncChangedCatalogue(status("failed", 120, 1), status("categories", 120, 1)), false);
});

test("the list keeps its length while a refresh is in flight", () => {
  // 5000 rows on screen, nothing fetched since the refresh: the estimate
  // alone (separators excluded) would shrink the scroll height under the
  // reader.
  assert.equal(refreshingTotal(4800, null, 5000), 5000);
  // A list that has grown takes the new length at once.
  assert.equal(refreshingTotal(5400, null, 5000), 5400);
  // No refresh under way: the ordinary projection.
  assert.equal(refreshingTotal(4800, null, null), 4800);
});

test("a short page proves the new end, shorter or not", () => {
  assert.equal(refreshingTotal(0, 1234, 5000), 1234);
  // An emptied catalogue empties the list rather than leaving old rows up.
  assert.equal(refreshingTotal(0, 0, 5000), 0);
});

test("an empty list that is refreshed grows to what arrived", () => {
  // The first-run case: nothing before, the categories now count 60000.
  assert.equal(refreshingTotal(60000, null, 0), 60000);
});

test("the movies phase writes to the mirror, so leaving it is a refresh", () => {
  const at = 1_700_000_000;
  assert.equal(syncChangedCatalogue(status("movies", 120, at), status("idle", 120, at)), true);
  assert.equal(syncChangedCatalogue(status("epg", 120, at), status("movies", 120, at)), true);
  assert.equal(syncChangedCatalogue(status("idle", 120, at), status("idle", 120, at)), false);
});

test("the shows phase writes to the mirror, so leaving it is a refresh", () => {
  const at = 1_700_000_000;
  assert.equal(syncChangedCatalogue(status("shows", 120, at), status("idle", 120, at)), true);
  assert.equal(syncChangedCatalogue(status("movies", 120, at), status("shows", 120, at)), true);
});
