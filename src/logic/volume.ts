// Volume, including above 100% — mpv's volume-max is set higher than 100 on
// purpose, because a lot of these streams are quiet.
//
// Checked in src/__checks__/volume.test.mjs.

export const VOLUME_MAX = 150;
export const VOLUME_STEP = 4;

/** Only the sign of a wheel event is trusted: deltaY arrives in pixels, lines
 *  or pages depending on the device, and scaling by the raw value makes a
 *  touchpad and a mouse behave completely differently. */
export function volumeAfterWheel(
  current: number,
  deltaY: number,
  step = VOLUME_STEP,
  max = VOLUME_MAX,
): number {
  if (deltaY === 0) return clamp(current, max);
  const direction = deltaY < 0 ? 1 : -1; // scroll up is louder
  return clamp(current + direction * step, max);
}

function clamp(v: number, max: number): number {
  return Math.min(max, Math.max(0, Math.round(v)));
}
