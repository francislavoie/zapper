// Whether the window can take the stacked layout: picture across the top at
// full width, guide underneath. Pure; exercised by
// src/__checks__/stack.test.mjs.
//
// The rule: offer it when the window is taller than it is wide
// and there is at least one and a half times the picture's own height left
// over for the guide. Below that the control does not appear at all, rather
// than appearing and producing something cramped — three visible guide rows
// under a letterbox is worse than the ordinary player.

/** The shape mpv will letterbox into. The picture's height follows from the
 *  width, which is the window's, because stacked means full width. */
export const STACK_ASPECT = 16 / 9;

/** How much room the guide needs below the picture, as a multiple of the
 *  picture's height. */
export const STACK_SPARE = 1.5;

export const STACK_STORAGE_KEY = "zapper.stacked";

/** The height a full-width picture takes in a window this wide. */
export function stackedVideoHeight(width: number): number {
  return width > 0 ? width / STACK_ASPECT : 0;
}

/** Is there room to stack?
 *
 *  `stageHeight` is everything below the app's own title bar, and the caller
 *  passes it rather than this working it out: video behind an opaque title bar
 *  is video nobody sees, and the bar's height belongs to the caller that can
 *  measure it.
 *
 *  Both tests are ratios of lengths in CSS pixels, so the answer does not
 *  change with page zoom: zoom scales the pixel, and every length here with
 *  it. */
export function canStack(
  windowWidth: number,
  windowHeight: number,
  stageHeight: number,
): boolean {
  if (!(windowWidth > 0) || !(windowHeight > 0) || !(stageHeight > 0)) return false;
  // "Taller than wide", exactly: a square window is not taller than wide.
  if (windowHeight <= windowWidth) return false;
  const video = stackedVideoHeight(windowWidth);
  if (video <= 0) return false;
  return stageHeight - video >= STACK_SPARE * video;
}

/** What local storage says, defaulting to off. Anything but the string this
 *  module writes is treated as absent — a corrupt value must not decide the
 *  layout. */
export function parseStacked(raw: string | null): boolean {
  return raw === "1";
}
