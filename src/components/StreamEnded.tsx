// What the picture says when the stream stopped by itself — most often
// because the provider handed the account's connection to another device —
// or while the backend reconnects by itself after the network failed.
//
// Drawn over the video, which nothing else may be (see WatchBand): there is
// no video under it to hide. mpv has stopped, and the slot would otherwise
// be an empty black box that says nothing.
//
// After the provider ended or refused the stream, tuning again is only ever
// a button. Where another device is the likely reason, the button says
// "Watch here anyway" and what it will do to that device; elsewhere it is
// Retry (see logic/ending.ts). A reconnect the backend is making by itself
// can be cancelled.

import { playChannel, playMovie } from "../actions";
import { backend } from "../backend";
import { endedActions, endedMessage, reconnectMessage, retryStart, retryTarget } from "../logic/ending";
import { run, useApp, useDispatch } from "../store";

export interface StreamEndedProps {
  /** The docked picture: smaller type, no detail line. */
  compact?: boolean;
  /** After Dismiss has stopped the player, for the view to step out of the
   *  player as stopping does. */
  onDismissed?: () => void;
}

export function StreamEnded({ compact = false, onDismissed }: StreamEndedProps) {
  const { player, nowChannel, movies } = useApp();
  const dispatch = useDispatch();
  const message = reconnectMessage(player) ?? endedMessage(player);
  const actions = endedActions(player);
  if (!message || !actions) return null;

  const retry = () => {
    const target = retryTarget(player);
    if (target === "none") return;
    if (target === "movie") {
      // The title the store knows, so the band and the marks follow; else by id.
      const id = player.onDemand?.id ?? -1;
      if (movies.playing?.movieId === id) void playMovie(dispatch, movies.playing, retryStart(player));
      else void run(dispatch, "retry", backend.playMovie(id, retryStart(player)));
      return;
    }
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
  const cancel = () => void run(dispatch, "cancel reconnect", backend.cancelReconnect());

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
        {actions.primary === "cancel" ? (
          <button className="ctl" onClick={cancel}>
            Cancel
          </button>
        ) : (
          <button className="ctl retry" onClick={retry} disabled={retryTarget(player) === "none"}>
            {actions.primary === "watchHere" ? "Watch here anyway" : "Retry"}
          </button>
        )}
        {actions.dismiss ? (
          <button className="ctl" onClick={dismiss}>
            Dismiss
          </button>
        ) : null}
      </div>
      {!compact && actions.note ? <p className="stream-ended-note">{actions.note}</p> : null}
    </div>
  );
}
