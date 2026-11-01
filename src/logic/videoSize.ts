// How big the docked picture is, as pure arithmetic.
//
// Expressed in rem rather than pixels so that it scales with application
// zoom for free: the stylesheet's --video-w is a rem value, and page zoom
// scales the rem. One number moves the slot, the band's height follows from
// the 16:9 ratio, and the rect handed to mpv is measured from the slot — so
// nothing else has to be told.
//
// A preference about this screen, not about the account, so it is stored
// locally alongside zoom and the stacked layout rather than in SettingsDto.
//
// Checked in src/__checks__/videoSize.test.mjs.

export const VIDEO_WIDTH_STORAGE_KEY = "zapper.videoWidth";

/** The design size, and what a reset returns to. */
export const VIDEO_WIDTH_DEFAULT = 22.25;

/** Below this the picture is too small to be worth the space it costs, and
 *  the "no picture" legend no longer fits inside it. */
export const VIDEO_WIDTH_MIN = 14;

/** Above this the docked picture stops being docked and starts being the
 *  layout.
 *
 *  Generous, because the information beside it wraps rather than being
 *  crushed: the picture may take most of the band and the facts rearrange
 *  into fewer columns. What it may not do is leave nothing, which is what
 *  this bound is for. */
export const VIDEO_WIDTH_MAX = 72;

export function clampVideoWidth(rem: number): number {
  if (!Number.isFinite(rem)) return VIDEO_WIDTH_DEFAULT;
  // Quarter-rem steps: fine enough that a drag feels continuous, coarse
  // enough that the stored value round-trips through a string exactly and
  // two drags to the same place give the same number.
  const stepped = Math.round(rem * 4) / 4;
  return Math.min(VIDEO_WIDTH_MAX, Math.max(VIDEO_WIDTH_MIN, stepped));
}

export function parseVideoWidth(raw: string | null): number {
  if (raw === null || raw === "") return VIDEO_WIDTH_DEFAULT;
  const n = Number.parseFloat(raw);
  return Number.isFinite(n) ? clampVideoWidth(n) : VIDEO_WIDTH_DEFAULT;
}

/** The width a drag lands on.
 *
 *  The slot is pinned to the right, so dragging its left edge leftwards — a
 *  negative delta — makes it wider. `pxPerRem` is measured from the slot
 *  itself rather than assumed to be 16: under CSS zoom a rem and a client
 *  pixel are different sizes, and measuring both in the same space is what
 *  makes this correct under either zoom mechanism without knowing which is
 *  in use. */
export function widthAfterDrag(
  startRem: number,
  deltaPx: number,
  pxPerRem: number,
): number {
  if (!(pxPerRem > 0) || !Number.isFinite(deltaPx)) return clampVideoWidth(startRem);
  return clampVideoWidth(startRem - deltaPx / pxPerRem);
}

/** The most of the stage's height the band may take. Past half, the list
 *  under it is the thing that has been squeezed out. */
export const VIDEO_BAND_SHARE = 0.5;

/** What has to stay beside the picture for the channel's name and programme
 *  to be readable, in rem. */
export const VIDEO_INFO_MIN = 18;

/** The widest picture this stage has room for, in rem, or `Infinity` when the
 *  stage has not been measured yet.
 *
 *  This is a ceiling on what is *drawn*, never on what is *remembered*. The
 *  stored width is the size somebody chose; a window opened smaller than last
 *  time draws less of it, and the full size comes back as soon as there is
 *  room again. Clamping the stored value instead meant one launch in a small
 *  window quietly threw the chosen size away. */
export function videoRoomRem(stageWidthRem: number, stageHeightRem: number): number {
  if (!(stageWidthRem > 0) || !(stageHeightRem > 0)) return Infinity;
  const byHeight = (stageHeightRem * VIDEO_BAND_SHARE * 16) / 9;
  const byWidth = stageWidthRem - VIDEO_INFO_MIN;
  // Down to the quarter: rounding up could hand back a width that does not
  // fit, and a value that moves in quarter steps re-renders only when it
  // has actually changed.
  return Math.floor(Math.min(byHeight, byWidth) * 4) / 4;
}

/** The width actually drawn: the chosen one, unless the stage cannot hold
 *  it. Never below the minimum, even in a window too small for that — a
 *  picture too small to see is not a smaller picture, it is a broken one. */
export function fittedVideoWidth(preferredRem: number, roomRem: number): number {
  return clampVideoWidth(Math.min(clampVideoWidth(preferredRem), roomRem));
}

/** Whether a width is the default, for the reset affordance. Not an equality
 *  test: the value round-trips through a string. */
export function isDefaultVideoWidth(rem: number): boolean {
  return Math.abs(clampVideoWidth(rem) - VIDEO_WIDTH_DEFAULT) < 1e-6;
}
