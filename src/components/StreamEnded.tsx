// What the picture says when the stream stopped by itself — most often
// because the provider handed the account's connection to another device.
//
// Drawn over the video, which nothing else may be (see WatchBand): there is
// no video under it to hide. mpv has stopped, and the slot would otherwise
// be an empty black box that says nothing.
//
// Retry is the only way back to the stream. Nothing reconnects on its own:
// a provider at its limit would end the other device's stream in turn (see
// logic/ending.ts).

import { playChannel } from "../actions";
import { backend } from "../backend";
import { endedMessage } from "../logic/ending";
import { run, useApp, useDispatch } from "../store";

export interface StreamEndedProps {
  /** The docked picture: smaller type, no detail line. */
  compact?: boolean;
  /** After Dismiss has stopped the player, for the view to step out of the
   *  player as stopping does. */
  onDismissed?: () => void;
}

export function StreamEnded({ compact = false, onDismissed }: StreamEndedProps) {
  const { player, nowChannel } = useApp();
  const dispatch = useDispatch();
  const message = endedMessage(player);
  if (!message) return null;

  const retry = () => {
    const id = player.streamId;
    if (id === null) return;
    // Through the same path as picking the channel, so the band paints its
    // metadata and the row highlights while it opens.
    if (nowChannel?.streamId === id) void playChannel(dispatch, nowChannel);
    else void run(dispatch, "retry", backend.play(id));
  };
  const dismiss = () => {
    void run(dispatch, "stop", backend.stop());
    onDismissed?.();
  };

  return (
    <div
      className={`stream-ended ${compact ? "compact" : ""}`}
      role="alert"
      // The picture behind treats a click as "open the player" or "resume";
      // a click on this panel is neither.
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
    >
      <p className="stream-ended-title">{message.title}</p>
      {!compact && message.detail ? <p className="stream-ended-detail">{message.detail}</p> : null}
      {!compact && message.code ? <p className="stream-ended-code">{message.code}</p> : null}
      <div className="stream-ended-actions">
        <button className="ctl retry" onClick={retry} disabled={player.streamId === null}>
          Retry
        </button>
        <button className="ctl" onClick={dismiss}>
          Dismiss
        </button>
      </div>
    </div>
  );
}
