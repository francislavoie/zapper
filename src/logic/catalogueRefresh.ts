// Noticing that a sync has changed the catalogue, and keeping what is on
// screen while the lists catch up with it.
//
// The categories, the channel pages and the guide are read from the local
// mirror and kept: the rail asks once, the list keeps every page it has
// fetched. A sync rewrites the mirror underneath them, and nothing about a
// kept page says it has gone stale — so without being told, the rail went on
// saying "No categories yet" and the list went on showing an empty page
// until the app was restarted. The sync's status events are what tells them.
//
// Checked in src/__checks__/catalogueRefresh.test.mjs.

import type { SyncPhase, SyncStatusDto } from "../types";

/** The phases during which a pass is writing to the mirror. Each one lands
 *  its part of the catalogue as it ends. */
const WRITING: ReadonlySet<SyncPhase> = new Set(["categories", "channels", "epg"]);

/** Whether the mirror may have changed between two sync statuses.
 *
 *  A phase lands its data as it ends, so leaving one — for the next phase,
 *  for `failed`, or for `idle` at the end of the pass — is the moment to read
 *  again. Not only the end of the pass: on a first sync the channels arrive
 *  minutes before the guide finishes, and the list should show them then.
 *
 *  A status that reports a different channel count or a newer pass says the
 *  same thing even when the phase changes in between were not seen. */
export function syncChangedCatalogue(prev: SyncStatusDto, next: SyncStatusDto): boolean {
  if (WRITING.has(prev.phase) && next.phase !== prev.phase) return true;
  if (next.channelsTotal !== prev.channelsTotal) return true;
  return next.lastSynced !== null && next.lastSynced !== prev.lastSynced;
}

/** How many rows the list should claim while a refresh is under way.
 *
 *  `fresh` is the projection from what has been fetched since the refresh,
 *  `freshExact` the end of the list if a fetch since then has proved it, and
 *  `stale` what the list claimed before the refresh, or `null` when nothing
 *  is being refreshed.
 *
 *  The old length is a floor until a new fetch proves the list ends sooner:
 *  dropping it while the first new pages are in flight would shrink the
 *  scroll height and throw the reader's place away, for a list that is
 *  usually the same length as before. Once a short page has said where the
 *  list ends, that is the answer, shorter or not. */
export function refreshingTotal(fresh: number, freshExact: number | null, stale: number | null): number {
  if (freshExact !== null) return Math.max(0, freshExact);
  return Math.max(fresh, stale ?? 0);
}
