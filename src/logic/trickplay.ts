// Previews on a movie's or an episode's seek bar.
//
// A title's previews are filed in slots a fixed spacing apart in its own
// time, at most one frame each, and the bar shows the frame of the slot under
// the pointer when there is one; elsewhere the time alone. The app's backend
// keeps them (crates/zapper/src/surface/trickplay.rs, by the same rules, which
// are spelled the same way here); a browser keeps the frames it has played
// itself (src/webVod.ts), with the rules below.

/** Seconds of a title between two slots. */
export const TITLE_SPACING_S = 10;

/** The most slots a title gets: a three-hour film at ten seconds is 1080. */
export const TITLE_MAX_SLOTS = 1100;

/** How close to a slot's moment a played frame is taken: the page looks
 *  four times a second, so a title playing passes within this of every
 *  slot. A slot seeked past is left for later, rather than filed with a
 *  frame from seconds away. */
export const CAPTURE_WINDOW_S = 1;

/** The spacing for a title `duration` seconds long: ten seconds, or a
 *  multiple of five wide enough to stay within TITLE_MAX_SLOTS. */
export function titleSpacing(duration: number | null): number {
  if (duration === null || !Number.isFinite(duration)) return TITLE_SPACING_S;
  if (duration <= TITLE_SPACING_S * TITLE_MAX_SLOTS) return TITLE_SPACING_S;
  return Math.ceil(duration / TITLE_MAX_SLOTS / 5) * 5;
}

/** The slot a moment belongs to: the nearest multiple of `spacing`. */
export function slotOf(at: number, spacing: number): number {
  // `|| 0`: a moment a hair before zero is slot 0, not slot -0.
  return Math.round(at / spacing) || 0;
}

/** The slot to take a played frame for now, or null: the picture is within
 *  CAPTURE_WINDOW_S of a slot's moment that has no frame yet. */
export function captureDue(
  frames: ReadonlyMap<number, string>,
  position: number,
  spacing: number,
): number | null {
  if (!Number.isFinite(position) || !(spacing > 0)) return null;
  const slot = slotOf(position, spacing);
  if (Math.abs(position - slot * spacing) > CAPTURE_WINDOW_S) return null;
  return frames.has(slot) ? null : slot;
}

/** `frames` with `frame` filed for `slot`, unless the slot has one or the
 *  title has all it may keep. */
export function keepFrame(
  frames: ReadonlyMap<number, string>,
  slot: number,
  frame: string,
): Map<number, string> {
  const next = new Map(frames);
  if (!next.has(slot) && next.size < TITLE_MAX_SLOTS) next.set(slot, frame);
  return next;
}

/** The frame of the slot nearest `at`, or null when it has none. */
export function titleFrameAt(
  frames: ReadonlyMap<number, string>,
  at: number,
  spacing: number,
): string | null {
  if (!Number.isFinite(at) || !(spacing > 0)) return null;
  return frames.get(slotOf(at, spacing)) ?? null;
}

/** Where a picture `w` x `h` goes inside a preview `tw` x `th`: as large as
 *  fits at its own shape, centred, the rest black. The whole preview for a
 *  picture of no known size. */
export function letterbox(
  w: number,
  h: number,
  tw: number,
  th: number,
): { x: number; y: number; width: number; height: number } {
  if (!(w > 0) || !(h > 0)) return { x: 0, y: 0, width: tw, height: th };
  const width = w * th >= h * tw ? tw : Math.max(1, Math.round((w * th) / h));
  const height = w * th >= h * tw ? Math.max(1, Math.round((h * tw) / w)) : th;
  return { x: Math.floor((tw - width) / 2), y: Math.floor((th - height) / 2), width, height };
}
