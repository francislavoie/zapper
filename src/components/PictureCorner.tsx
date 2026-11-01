// The top-right corner of the picture, where things that happened to the
// picture are said: a channel number being typed, and playback skipping
// ahead to live — by itself, or because Live was pressed, where how far it
// went is the answer to "how far behind was I?".
//
// Drawn in the controls' layer the way the gesture bubbles are — inside it,
// outside its two bars — so it is seen while the controls are faded, and it
// takes no pointer events, so nothing under it stops answering. One column,
// so the two can be up at once without landing on each other: tuning by
// number is followed, a few seconds later, by exactly that skip.

import { duration } from "../logic/format";
import { useApp } from "../store";
import { ZapEntry } from "./ZapEntry";

export function PictureCorner({ docked = false }: { docked?: boolean }) {
  const { zap, liveJump, mode } = useApp();
  // The docked picture is too small to type over; the chip says it there.
  // Watching includes the stacked layout, whose picture is a full one.
  const showZap = zap !== null && !docked && mode === "watch";
  if (!showZap && !liveJump) return null;

  return (
    <div className="pic-corner" aria-live="polite">
      {showZap ? <ZapEntry zap={zap} /> : null}
      {liveJump ? (
        // Keyed by the jump, so a second one restarts the entrance rather
        // than changing a number in place.
        <div key={liveJump.seq} className="live-jump" role="status">
          <span className="live-dot" aria-hidden="true" />
          <b>LIVE</b>
          <span className="live-jump-by">
            Jumped {duration(liveJump.seconds)}
            <span className="live-jump-tail"> to live</span>
          </span>
        </div>
      ) : null}
    </div>
  );
}
