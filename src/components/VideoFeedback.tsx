// The bubbles that answer a gesture over the picture.
//
// Modelled on YouTube's: a round translucent slug that appears where the
// gesture landed, pops once and fades, with no layout of its own and no
// pointer events. It exists because the gestures are invisible — scrolling
// changes a number nothing on screen is showing, and a double-click seek
// looks identical to a stream hiccup unless something says otherwise.
//
// Every bubble carries an `id`. React keys on it, so a repeat of the same
// gesture remounts the element and the CSS animation plays again from the
// start rather than sitting finished — which is what makes a run of wheel
// notches read as a run of ticks instead of one bubble that never moves.

import { useFeedback, type Feedback } from "../hooks/useFeedback";
import { zoneCentre } from "../logic/gestures";
import { SkipBack, SkipForward, Volume } from "./icons";

export type { Feedback, FeedbackKind } from "../hooks/useFeedback";

export function VideoFeedback({ feedback }: { feedback: Feedback | null }) {
  const shown = useFeedback(feedback);
  if (!shown) return null;

  const centre = shown.kind === "volume" ? 0.5 : zoneCentre(shown.kind);

  return (
    // aria-hidden because this is a picture of something that already
    // happened: the control that changed says so itself, and a screen reader
    // announcing a decorative bubble would be announcing it twice.
    <div className="vfb-layer" aria-hidden="true">
      <div
        key={shown.id}
        className={`vfb vfb-${shown.kind}`}
        style={{ left: `${centre * 100}%` }}
      >
        <span className="vfb-icon">
          {shown.kind === "volume" ? (
            <Volume className="i" level={shown.level ?? "high"} />
          ) : shown.kind === "back" ? (
            <SkipBack className="i" seconds={shown.seconds ?? 5} showSeconds={false} />
          ) : (
            <SkipForward className="i" seconds={shown.seconds ?? 5} showSeconds={false} />
          )}
        </span>
        <span className="vfb-label">{shown.label}</span>
      </div>
    </div>
  );
}
