// The gestures over a picture — a double-click in one of its three zones, a
// wheel notch — and the bubbles that answer them. One implementation for the
// full-size player and the docked picture, so a double-click at the edge of
// either does the same thing and says so the same way.
//
// The zones and what they mean are in logic/gestures.ts; what "fullscreen"
// means is the caller's, because it differs: full size it toggles, docked it
// goes through the player to get there.

import { useCallback, useRef, useState } from "react";
import { seekBy } from "../actions";
import { SKIP_SECONDS, zoneAction, zoneAt } from "../logic/gestures";
import { volumeLevel } from "../logic/volume";
import { useDispatch } from "../store";
import type { Feedback } from "./useFeedback";

export interface VideoGestureOptions {
  atLiveEdge: boolean;
  muted: boolean;
  /** A double-click in the middle zone. */
  onFullscreen: () => void;
  /** Touching the picture is not idle; keeps the controls up. */
  onActivity: () => void;
  /** Seconds a side zone seeks by; a movie's is longer than a channel's. */
  step?: number;
}

export function useVideoGestures({
  atLiveEdge,
  muted,
  onFullscreen,
  onActivity,
  step = SKIP_SECONDS,
}: VideoGestureOptions) {
  const dispatch = useDispatch();

  // The gestures change things nothing on screen is showing, so each one
  // raises a bubble. The counter is what makes a repeat visible: an
  // identical gesture produces an identical description, and without a
  // changing id React would reuse the element and the animation would not
  // replay.
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const feedbackId = useRef(0);
  const flash = useCallback((f: Omit<Feedback, "id">) => {
    feedbackId.current += 1;
    setFeedback({ ...f, id: feedbackId.current });
  }, []);

  /** The bubble for a wheel notch that has set the volume to `next`.
   *  `mutedAfter` is for a caller whose wheel also unmutes. */
  const flashVolume = (next: number, mutedAfter = muted) =>
    flash({ kind: "volume", label: `${next}%`, level: volumeLevel(next, mutedAfter) });

  /** A double-click at `clientX` over a picture whose box is `box`. */
  const doubleClick = (clientX: number, box: { left: number; width: number }) => {
    const action = zoneAction(zoneAt(clientX, box.left, box.width), atLiveEdge, step);
    // Forward at the live edge has nowhere to go. Saying nothing is better
    // than a bubble promising five seconds that do not exist.
    if (!action) return;
    if (action.kind === "fullscreen") {
      onFullscreen();
      return;
    }
    onActivity();
    flash({ kind: action.zone, label: `${step} seconds`, seconds: step });
    void seekBy(dispatch, action.by);
  };

  return { feedback, flashVolume, doubleClick };
}
