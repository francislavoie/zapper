// What a double-click on the picture means, and where its bubble belongs.
//
// Three zones across the width: seek back on the left, seek forward on the
// right, fullscreen in the middle. The side zones are deliberately the
// minority of the width — fullscreen is the gesture people reach for most, and
// a mis-hit that jumps the stream is more annoying than one that does not.
//
// A single click never pauses. This is live TV, and click-to-pause is a VOD
// idiom that misfires every time a click meant for the window lands on the
// video. It does *resume* a paused picture (PlayerChrome, WatchBand): that
// direction cannot misfire, since a paused picture is waiting to be resumed.
//
// Checked in src/__checks__/gestures.test.mjs.

export type VideoZone = "back" | "middle" | "forward";

/** How much of the width each seek zone takes, from its own edge.
 *
 *  Narrow on purpose. Fullscreen is the gesture people reach for most, a
 *  mis-hit that toggles it undoes itself with the same gesture, and a
 *  mis-hit that seeks does not — so the middle keeps the overwhelming
 *  majority of the picture and the sides are corners you have to mean. */
export const SEEK_ZONE_FRACTION = 0.15;

export function zoneAtFraction(fraction: number): VideoZone {
  // A pointer outside the box, or a box with no width, is not a seek. Falling
  // back to the middle means the worst case is a fullscreen toggle, which is
  // undoable with the same gesture.
  if (!Number.isFinite(fraction)) return "middle";
  if (fraction < SEEK_ZONE_FRACTION) return "back";
  if (fraction > 1 - SEEK_ZONE_FRACTION) return "forward";
  return "middle";
}

/** The zone for a pointer at `clientX` over a box starting at `left`. */
export function zoneAt(clientX: number, left: number, width: number): VideoZone {
  if (!(width > 0)) return "middle";
  return zoneAtFraction((clientX - left) / width);
}

/** Where a zone's bubble is centred, as a fraction of the width.
 *
 *  Centred within its own region rather than under the pointer: the bubble is
 *  feedback about which zone was hit, so it should land in the same place
 *  every time and teach where the zones are. */
export function zoneCentre(zone: VideoZone): number {
  switch (zone) {
    case "back":
      return SEEK_ZONE_FRACTION / 2;
    case "forward":
      return 1 - SEEK_ZONE_FRACTION / 2;
    case "middle":
      return 0.5;
  }
}

/** Which way a zone seeks, in seconds, given the step. `middle` does not seek. */
export function zoneSeek(zone: VideoZone, step: number): number | null {
  switch (zone) {
    case "back":
      return -step;
    case "forward":
      return step;
    case "middle":
      return null;
  }
}

/** Whether a double-click in this zone does anything.
 *
 *  Seeking forward at the live edge has nowhere to go, and a bubble promising
 *  five seconds that do not exist is worse than no bubble at all. */
export function zoneIsAvailable(zone: VideoZone, atLiveEdge: boolean): boolean {
  return zone !== "forward" || !atLiveEdge;
}
