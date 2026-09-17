// How wide the category/favourites rail is, as pure arithmetic.
//
// The same shape as the docked picture's width and for the same reasons: in
// rem so it scales with application zoom for free, quantised so the stored
// value round-trips through a string exactly, and local rather than synced
// because it is a property of this screen.
//
// The one difference is direction. The picture is pinned to the right edge
// and grows leftwards; the rail is pinned to the left and grows rightwards,
// so a positive drag makes it wider.
//
// Checked in src/__checks__/railWidth.test.mjs.

export const RAIL_WIDTH_STORAGE_KEY = "zapper.railWidth";

/** The design size, and what a reset returns to. */
export const RAIL_WIDTH_DEFAULT = 16.25;

/** Narrower than this and the category names are all ellipsis, which makes
 *  the rail useless rather than compact. */
export const RAIL_WIDTH_MIN = 9;

/** Wider than this and the channel list — the thing the rail is for
 *  navigating — stops having room to be a list. */
export const RAIL_WIDTH_MAX = 34;

export function clampRailWidth(rem: number): number {
  if (!Number.isFinite(rem)) return RAIL_WIDTH_DEFAULT;
  const stepped = Math.round(rem * 4) / 4;
  return Math.min(RAIL_WIDTH_MAX, Math.max(RAIL_WIDTH_MIN, stepped));
}

export function parseRailWidth(raw: string | null): number {
  if (raw === null || raw === "") return RAIL_WIDTH_DEFAULT;
  const n = Number.parseFloat(raw);
  return Number.isFinite(n) ? clampRailWidth(n) : RAIL_WIDTH_DEFAULT;
}

/** The width a drag lands on.
 *
 *  `pxPerRem` is measured from the rail itself rather than assumed to be 16:
 *  under CSS zoom a rem and a client pixel are different sizes, and taking
 *  both from the same box is what makes the edge track the pointer under
 *  either zoom mechanism. */
export function railAfterDrag(startRem: number, deltaPx: number, pxPerRem: number): number {
  if (!(pxPerRem > 0) || !Number.isFinite(deltaPx)) return clampRailWidth(startRem);
  return clampRailWidth(startRem + deltaPx / pxPerRem);
}

export function isDefaultRailWidth(rem: number): boolean {
  return Math.abs(clampRailWidth(rem) - RAIL_WIDTH_DEFAULT) < 1e-6;
}
