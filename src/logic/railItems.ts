// What each id on the Live TV rail means. The rail's own items (Favourites,
// Recently watched, Recordings, All channels) and the provider's categories
// share one selection string, so everything that already follows "the
// selected category" (the channel list, the guide, going to a channel) keeps
// working; the starred ids below can never collide with a provider's id.
// Pure; exercised by src/__checks__/railItems.test.mjs.

import type { FavouriteDto } from "../types";
import { focusCategory } from "./programmes.ts";

export const ALL_CATEGORIES = "*all*";
export const FAVOURITES = "*favourites*";
export const RECENT = "*recent*";
export const RECORDINGS = "*recordings*";

export type RailItemKind = "favourites" | "recent" | "recordings" | "all" | "category";

export function railItemKind(id: string): RailItemKind {
  switch (id) {
    case FAVOURITES:
      return "favourites";
    case RECENT:
      return "recent";
    case RECORDINGS:
      return "recordings";
    case ALL_CATEGORIES:
      return "all";
    default:
      return "category";
  }
}

/** The `categoryId` to page channels with: null for every channel, the id for
 *  a category, undefined for a list the backend does not page. */
export function pagedCategory(id: string): string | null | undefined {
  const kind = railItemKind(id);
  if (kind === "all") return null;
  return kind === "category" ? id : undefined;
}

/** Recordings has no guide of its own, so a guide asked for there shows every
 *  channel. */
export function guideSelection(id: string): string {
  return id === RECORDINGS ? ALL_CATEGORIES : id;
}

/** The selection after the mode changes. */
export function modeSelection(id: string, mode: string): string {
  return mode === "guide" ? guideSelection(id) : id;
}

export function selectionTitle(
  id: string,
  categories: readonly { id: string; name: string }[],
): string {
  switch (railItemKind(id)) {
    case "favourites":
      return "Favourites";
    case "recent":
      return "Recently watched";
    case "recordings":
      return "Recordings";
    case "all":
      return "All channels";
    default:
      return categories.find((c) => c.id === id)?.name ?? "Channels";
  }
}

export function selectionSubtitle(id: string, count: number): string {
  const channels = `${count.toLocaleString()} ${count === 1 ? "channel" : "channels"}`;
  // Nothing yet (the first run): the order alone, without a count of zero.
  switch (railItemKind(id)) {
    case "favourites":
      return count === 0 ? "most watched first" : `${channels}, most watched first`;
    case "recent":
      return count === 0 ? "newest first" : `${channels}, newest first`;
    default:
      return channels;
  }
}

/** How many channels a selection will hold, before its pages have loaded.
 *  Callers pass no categories while they are not ready; All then falls back
 *  to the sync's running total. */
export function selectionEstimate(
  id: string,
  categories: readonly { id: string; channelCount: number }[],
  syncTotal: number,
): number {
  const kind = railItemKind(id);
  if (kind === "all") {
    return categories.length === 0 ? syncTotal : categories.reduce((n, c) => n + c.channelCount, 0);
  }
  if (kind === "category") return categories.find((c) => c.id === id)?.channelCount ?? 0;
  return 0;
}

/** The stream ids a favourites or recent selection lists, else null. */
export function listedFor(
  id: string,
  favourites: readonly FavouriteDto[] | null,
  recent: readonly FavouriteDto[] | null,
): readonly number[] | null {
  const kind = railItemKind(id);
  const list = kind === "favourites" ? favourites : kind === "recent" ? recent : null;
  return list ? list.map((f) => f.channel.streamId) : null;
}

export interface FocusTarget {
  categoryId: string;
  /** The channel's place in the list when it is already on screen. */
  index: number | null;
  /** Whether its place has to be looked up in the backend. */
  lookUp: boolean;
}

/** Where going to a channel lands: the favourites or recent list when the
 *  channel is in it, otherwise the usual category choice. */
export function focusTarget(
  current: string,
  channel: { streamId: number; categoryId: string | null },
  listed: readonly number[] | null,
): FocusTarget {
  const at = listed ? listed.indexOf(channel.streamId) : -1;
  if (at >= 0) return { categoryId: current, index: at, lookUp: false };
  return {
    categoryId: focusCategory(current, channel.categoryId, ALL_CATEGORIES),
    index: null,
    lookUp: true,
  };
}

/** Continue watching's id on both tabs: starred, so never a provider category's. */
export const CONTINUE = "*continue*";

export interface FixedRailItem {
  id: string;
  name: string;
  count: string;
}

const countText = (n: number | null) => (n ? n.toLocaleString() : "");

/** The rail's own items, in order. A count is blank until there is something
 *  to count. */
export function fixedRailItems(c: {
  favourites: number | null;
  recent: number | null;
  recordings: string;
  total: number;
}): FixedRailItem[] {
  return [
    { id: FAVOURITES, name: "Favourites", count: countText(c.favourites) },
    { id: RECENT, name: "Recently watched", count: countText(c.recent) },
    { id: RECORDINGS, name: "Recordings", count: c.recordings },
    { id: ALL_CATEGORIES, name: "All channels", count: countText(c.total) },
  ];
}
