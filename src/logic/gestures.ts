// What a double-click on the picture means, and where its bubble belongs.
//
// Three zones across the width: seek back on the left, seek forward on the
// right, fullscreen in the middle. The side zones are deliberately the
// minority of the width — fullscreen is the gesture people reach for most, and
// a mis-hit that jumps the stream is more annoying than one that does not.
//
// On a channel a single click never pauses. This is live TV, and
// click-to-pause is a VOD idiom that misfires every time a click meant for
// the window lands on the video. It does *resume* a paused picture
// (PlayerChrome, WatchBand): that direction cannot misfire, since a paused
// picture is waiting to be resumed. A movie is the VOD case the idiom is
// for, so there a click on the picture toggles pause.
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

/** What the skip controls and the side zones move by, and what their icons
 *  say. One number for the bar, the docked controls and the gestures, so an
 *  arrow can never claim a different jump from the one it makes. */
export const SKIP_SECONDS = 5;

export type ZoneAction =
  | { kind: "fullscreen" }
  | { kind: "seek"; zone: "back" | "forward"; by: number }
  | null;

/** What a double-click in a zone does, full size and docked alike.
 *
 *  The middle is fullscreen in both places. Docked, that means all the way
 *  to fullscreen (through the player), the same destination as the docked
 *  bar's fullscreen button: a single click already goes to the player, so
 *  the double-click is spent on the further place rather than repeating it. */
export function zoneAction(zone: VideoZone, atLiveEdge: boolean, step = SKIP_SECONDS): ZoneAction {
  if (zone === "middle") return { kind: "fullscreen" };
  if (!zoneIsAvailable(zone, atLiveEdge)) return null;
  const by = zoneSeek(zone, step);
  return by === null ? null : { kind: "seek", zone, by };
}

/** How long a click on the docked picture waits to find out whether it is
 *  the first half of a double-click.
 *
 *  It has to wait. A single click leaves for the player, which unmounts the
 *  docked picture, so without the wait the second click of a double-click
 *  lands on the full-size player — at a different place in its zones, and
 *  usually in its seek-forward corner, since the docked picture is top
 *  right. Short, because every single click pays it. */
export const DOUBLE_CLICK_MS = 250;

/** Which half of a click sequence a click is, from `MouseEvent.detail`.
 *  Zero is a click with no pointer count behind it (synthesised), which is
 *  a single click. A third click in a row is not a new gesture. */
export function clickIntent(detail: number): "single" | "double" | "ignore" {
  if (!(detail >= 2)) return "single";
  return detail === 2 ? "double" : "ignore";
}

/** What a single click (or Enter) on the docked picture does.
 *
 *  Paused, it resumes where it is rather than going full screen: the picture
 *  is frozen, and getting it moving is the obvious thing being asked for.
 *  Otherwise, while there is a stream, it goes to the player; a channel that
 *  was only restored is started; and with neither there is nothing to do. */
export function slotAction(
  state: "stopped" | "opening" | "playing" | "paused" | "failed",
  ready: boolean,
): "resume" | "watch" | "start" | null {
  if (state === "paused") return "resume";
  if (state !== "stopped") return "watch";
  return ready ? "start" : null;
}
