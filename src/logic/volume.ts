// Volume, including above 100% — mpv's `volume-max` is 200 on purpose,
// because a lot of these streams are quiet.
//
// The slider is not linear, and that is the point. If 0-200 were spread evenly
// across the track then "normal" would sit in the middle, and every stream
// would get nudged into software boost by accident. Instead the sane range,
// 0-100, owns most of the track and the boost above it is squeezed into the
// last stretch, so crossing 100 is a deliberate move into a region the control
// also paints differently.
//
// Checked in src/__checks__/volume.test.mjs.

export const VOLUME_MAX = 200;
/** 100%: no software gain, the loudest the stream is without help. */
export const VOLUME_UNITY = 100;
export const VOLUME_STEP = 5;

/** How much of the slider's length the 0-100 range occupies. The remaining
 *  fraction carries 100-200, so a boost decibel costs more travel than a
 *  normal one. */
export const UNITY_FRACTION = 0.7;

const clampTo = (v: number, max: number): number =>
  Math.min(max, Math.max(0, Math.round(v)));

const clamp01 = (n: number): number => Math.min(1, Math.max(0, n));

/** Only the sign of a wheel event is trusted: deltaY arrives in pixels, lines
 *  or pages depending on the device, and scaling by the raw value makes a
 *  touchpad and a mouse behave completely differently. */
export function volumeAfterWheel(
  current: number,
  deltaY: number,
  step = VOLUME_STEP,
  max = VOLUME_MAX,
): number {
  if (deltaY === 0) return clampTo(current, max);
  const direction = deltaY < 0 ? 1 : -1; // scroll up is louder
  return clampTo(current + direction * step, max);
}

/** A ceiling at or below unity has no boost region to squeeze, so the split
 *  degenerates to a plain linear track rather than dividing by zero. */
const split = (max: number, unityFraction: number): number =>
  max > VOLUME_UNITY ? Math.min(0.99, Math.max(0.01, unityFraction)) : 1;

/** Where the handle sits, 0-1 along the track, for a given volume. */
export function positionOfVolume(
  volume: number,
  max = VOLUME_MAX,
  unityFraction = UNITY_FRACTION,
): number {
  const f = split(max, unityFraction);
  const v = Math.min(max, Math.max(0, volume));
  if (f >= 1 || v <= VOLUME_UNITY) return clamp01((v / VOLUME_UNITY) * f);
  return clamp01(f + ((v - VOLUME_UNITY) / (max - VOLUME_UNITY)) * (1 - f));
}

/** The volume a point on the track means. Integer, because volume is reported
 *  and displayed as one and a slider that sets 43.7188% would make the readout
 *  disagree with the handle. */
/** How close a pointer has to land to 100% to mean 100%.
 *
 *  Unity is the value people come back to, and it is a single point on a
 *  track where every other position is as good as its neighbour — so
 *  without this, returning to "normal" is a pixel-hunt that usually lands
 *  on 99 or 101.
 *
 *  In volume units rather than pixels, because that is what the arithmetic
 *  here speaks and it is what a test can pin. The translation: 0-100 spans
 *  70% of the track, so eight units is about eleven pixels either side on
 *  the player bar's slider and six on the docked picture's shorter one.
 *  Both are targets you can hit without aiming, which four — three pixels
 *  on the short one — was not.
 *
 *  The cost is that 92 to 108 cannot be *dragged* to, only wheeled or
 *  typed. Nobody wants 96 rather than 100 badly enough to mind, and the
 *  wheel still reaches it.
 *
 *  Only the pointer snaps. The wheel and the arrow keys step by a known
 *  amount from where they are, and a step that silently became a different
 *  step would be worse than a pixel-hunt. */
export const UNITY_SNAP = 8;

export function volumeAtPosition(
  position: number,
  max = VOLUME_MAX,
  unityFraction = UNITY_FRACTION,
): number {
  const f = split(max, unityFraction);
  const p = clamp01(position);
  const raw =
    f >= 1 || p <= f
      ? (p / f) * VOLUME_UNITY
      : VOLUME_UNITY + ((p - f) / (1 - f)) * (max - VOLUME_UNITY);
  if (Math.abs(raw - VOLUME_UNITY) <= UNITY_SNAP) return clampTo(VOLUME_UNITY, max);
  return clampTo(raw, max);
}

/** Above unity is software gain, and the control says so rather than leaving
 *  it to be discovered by distortion. */
export function isBoosted(volume: number): boolean {
  return volume > VOLUME_UNITY;
}

/** The readout beside the slider. Muting wins over the number: what you want
 *  to know from a muted player is that it is muted, not what it would be. */
export function volumeLabel(volume: number, muted: boolean): string {
  if (muted) return "muted";
  return `${Math.round(volume)}%`;
}

/** Which of the icon's three states matches. Zero volume reads as muted even
 *  when the mute flag is off, because it sounds identical. */
export function volumeLevel(
  volume: number,
  muted: boolean,
): "muted" | "low" | "high" {
  if (muted || volume <= 0) return "muted";
  return volume < VOLUME_UNITY / 2 ? "low" : "high";
}
