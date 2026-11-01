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

/** A sanity bound on a stored or dragged width, not a layout limit.
 *
 *  How big the picture may be drawn is the window's business — see
 *  `videoRoomRem`, which leaves the information beside it its column and the
 *  list below it its rows, however large the window. A fixed ceiling here
 *  held a maximised picture well short of that on any large screen. This
 *  only keeps nonsense out of storage: 240rem is 3840 pixels at 100%, wider
 *  than any picture a window could draw. */
export const VIDEO_WIDTH_MAX = 240;

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

/** What has to stay under the band for the list to be usable, in rem: four
 *  or five rows. A fixed strip rather than a share of the height — capping
 *  the band at half the stage stopped a big window from having a big
 *  picture, which is the whole reason to drag it bigger. */
export const VIDEO_LIST_MIN = 12;

/** What has to stay beside the picture for the information in it to read,
 *  in rem: the picture grows until the panel would be narrower than this.
 *
 *  The panel's columns wrap into one as it narrows, so what has to fit is
 *  its widest column at the width it was designed for: the facts column at
 *  its full 18.75rem basis, the width at which the measured line ("1920×1080
 *  · 59.94 fps · hevc") is never cut short — the one line in the panel that
 *  must not be. Plus the panel's 1rem of padding either side, and a little
 *  over for its border and for a name or programme title longer than the
 *  line it sits beside. At this width the name keeps its Full screen and
 *  Stop buttons on its own line and the now/next titles take a line each.
 *  The picture is big by then: in a maximised 1920×1080 window the list's
 *  strip below runs out first, with the picture about 94rem wide. */
export const VIDEO_INFO_MIN = 22;

/** The resize handle between the panel and the picture, in rem. Mirrors
 *  `.watch-grip`'s width in styles.css, which the checks tie it to: it takes
 *  its share of the band whatever size the picture is. */
export const VIDEO_GRIP_W = 0.875;

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
  const byHeight = ((stageHeightRem - VIDEO_LIST_MIN) * 16) / 9;
  // The picture keeps 16:9, so whichever of these is the tighter decides it:
  // the band may not push the list under its strip, and the picture may not
  // squeeze the panel beside it below its minimum.
  const byWidth = stageWidthRem - VIDEO_INFO_MIN - VIDEO_GRIP_W;
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
