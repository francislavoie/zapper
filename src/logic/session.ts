// What the app was doing last time, as pure parsing.
//
// Where you were is not a setting. It is not synced, it does not belong to
// the account, and two machines signed into the same account should each come
// back to their own place — the same reasoning as zoom and the stacked
// layout. So it is local storage, and the side-effecting half lives in
// src/session.ts.
//
// Everything here is defensive about what it reads back. The value can be
// hand-edited, can come from an older build with a different shape, and can
// be truncated by a browser reclaiming space. A restored position that is
// nonsense should put the app at the top of the list, never leave it blank
// or throw on the way up.
//
// Checked in src/__checks__/session.test.mjs.

export const SESSION_STORAGE_KEY = "zapper.session";

export interface Session {
  /** ALL_CATEGORIES for the pseudo-entry, else a CategoryDto.id. */
  categoryId: string | null;
  railTab: "categories" | "favourites";
  /** Pixels, in layout units — the same units the lists scroll in. */
  railScrollTop: number;
  channelScrollTop: number;
  /** The channel to have ready, not to play. */
  lastStreamId: number | null;
  /** The channel before it, for P. Kept for the same reason the channel is:
   *  a remote's recall still goes back after the set was off, and the app
   *  comes back showing the channel, so flipping away from it and back
   *  should work on the first press rather than only after a second zap. */
  previousStreamId: number | null;
  /** Whether the loudness compressor was left on. A taste about how loud
   *  things should be, which does not change between launches — leaving it
   *  off every time means turning it on every time. */
  compressor: boolean;
  /** Whether the categories/favourites rail is folded away, in the list and
   *  the guide alike. A layout taste, kept between launches. */
  railCollapsed: boolean;
  /** The volume the app was closed at, in the slider's own 0–200 units, or
   *  null when none was ever set (then mpv's default stands). */
  volume: number | null;
  muted: boolean;
}

/** The slider's range, which is also what is trusted back from storage. */
const VOLUME_CEILING = 200;

export const EMPTY_SESSION: Session = {
  categoryId: null,
  railTab: "categories",
  railScrollTop: 0,
  channelScrollTop: 0,
  lastStreamId: null,
  previousStreamId: null,
  compressor: false,
  railCollapsed: false,
  volume: null,
  muted: false,
};

const volume = (v: unknown): number | null =>
  typeof v === "number" && Number.isFinite(v)
    ? Math.min(VOLUME_CEILING, Math.max(0, v))
    : null;

const streamId = (v: unknown): number | null =>
  typeof v === "number" && Number.isInteger(v) ? v : null;

const scroll = (v: unknown): number =>
  typeof v === "number" && Number.isFinite(v) && v > 0 ? Math.floor(v) : 0;

export function parseSession(raw: string | null): Session {
  if (raw === null || raw === "") return { ...EMPTY_SESSION };
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    // Truncated or hand-mangled. Starting fresh is the only safe reading.
    return { ...EMPTY_SESSION };
  }
  if (typeof value !== "object" || value === null) return { ...EMPTY_SESSION };
  const v = value as Record<string, unknown>;
  return {
    // A category that no longer exists is caught when the rail loads, not
    // here: this cannot know the catalogue.
    categoryId: typeof v.categoryId === "string" && v.categoryId !== "" ? v.categoryId : null,
    railTab: v.railTab === "favourites" ? "favourites" : "categories",
    railScrollTop: scroll(v.railScrollTop),
    channelScrollTop: scroll(v.channelScrollTop),
    lastStreamId: streamId(v.lastStreamId),
    previousStreamId: streamId(v.previousStreamId),
    compressor: v.compressor === true,
    railCollapsed: v.railCollapsed === true,
    volume: volume(v.volume),
    muted: v.muted === true,
  };
}

export function serialiseSession(s: Session): string {
  return JSON.stringify({
    categoryId: s.categoryId,
    railTab: s.railTab,
    railScrollTop: scroll(s.railScrollTop),
    channelScrollTop: scroll(s.channelScrollTop),
    lastStreamId: s.lastStreamId,
    previousStreamId: s.previousStreamId,
    compressor: s.compressor,
    railCollapsed: s.railCollapsed,
    volume: volume(s.volume),
    muted: s.muted,
  });
}
