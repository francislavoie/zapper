// Wheel scrolling that moves at once instead of gliding.
//
// WebKitGTK animates every wheel event over several frames, and since 2.4x
// its threaded scrolling ignores the `enable-smooth-scrolling` setting the
// desktop app turns off for
// scrollable boxes like the category rail and the channel list. One notch
// reads as the list lagging the wheel; a fast spin of a free-spinning wheel
// is worse, because each new event restarts the glide from wherever the last
// one had got to and the list crawls a fraction of the distance spun.
//
// So the page takes vertical wheel events itself and moves the box by the
// event's own distance, immediately. Every event, not only whole notches:
// high-resolution and free-spinning wheels report fractional steps that look
// exactly like a touchpad's, so there is no reliable way to tell them apart.
// The cost is that a touchpad tracks the fingers 1:1 but stops when they
// lift, rather than coasting.
//
// Pure; checked in src/__checks__/wheel.test.mjs.

export interface WheelLike {
  deltaX: number;
  deltaY: number;
  deltaMode: number;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  defaultPrevented: boolean;
}

/** Whether the page should apply this wheel event itself. */
export function takesWheel(e: WheelLike): boolean {
  if (e.defaultPrevented) return false; // someone closer already used it
  if (e.ctrlKey || e.metaKey) return false; // zoom
  if (e.shiftKey) return false; // horizontal scrolling, left to the engine
  // Mostly-horizontal gestures (a touchpad's sideways swipe) stay native.
  return e.deltaY !== 0 && Math.abs(e.deltaY) >= Math.abs(e.deltaX);
}

/** Pixels to move. Lines and pages are converted with the sizes engines use,
 *  though WebKitGTK sends pixels in practice. */
export function stepPixels(e: Pick<WheelLike, "deltaY" | "deltaMode">, clientHeight: number): number {
  switch (e.deltaMode) {
    case 1:
      return e.deltaY * 40;
    case 2:
      return e.deltaY * clientHeight;
    default:
      return e.deltaY;
  }
}
