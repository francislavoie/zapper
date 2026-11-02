// Record from the player: the programme on now on the channel playing, from
// the picture's menu and the now-playing panel (and R, useGlobalKeys). Once
// it records, both say so and offer to stop it; while it is saved into its
// file, they say that.
//
// On the desktop the app keeps what the player has already received of the
// programme, and on an account with no connection to spare moves the
// player onto the recording service's connection, which the recording
// shares (docs/recording.md, "Recording what you watch"; ipc::handover). In
// a browser the picture already comes through the service, so the
// recording simply joins it.

import { cancelRecording, recordFromPlayer } from "../actions";
import type { PlayerRecord } from "../logic/recordings";
import { useDispatch } from "../store";

/** The picture menu's rows. */
export function PlayerRecordItems({ record, onClose }: { record: PlayerRecord; onClose: () => void }) {
  const dispatch = useDispatch();
  switch (record.kind) {
    case "record":
      return (
        <button
          className="tb-menu-item"
          role="menuitem"
          onClick={() => {
            onClose();
            void recordFromPlayer(dispatch, record);
          }}
        >
          <span className="ctx-check">
            <span className="rec-dot" aria-hidden="true" />
          </span>
          <span className="tb-menu-label" data-tip-overflow="">
            Record “{record.programme.title}”
          </span>
          <span className="tb-menu-hint">R</span>
        </button>
      );
    case "saving":
      return (
        <button className="tb-menu-item" role="menuitem" disabled>
          <span className="ctx-check">
            <span className="rec-dot saving" aria-hidden="true" />
          </span>
          <span className="tb-menu-label">Saving the recording…</span>
        </button>
      );
    case "recording":
    case "scheduled": {
      const recording = record.kind === "recording";
      return (
        <>
          <div className="tb-menu-item ctx-status" role="none">
            <span className="ctx-check">
              <span className={`rec-dot ${recording ? "live" : ""}`} aria-hidden="true" />
            </span>
            <span className="tb-menu-label" data-tip-overflow="">
              {recording ? "Recording" : "Will record"} “{record.rec.title}”
            </span>
          </div>
          <button
            className="tb-menu-item"
            role="menuitem"
            onClick={() => {
              onClose();
              void cancelRecording(dispatch, record.rec);
            }}
          >
            <span className="ctx-check" />
            <span className="tb-menu-label">{recording ? "Stop recording" : "Cancel recording"}</span>
          </button>
        </>
      );
    }
  }
}

/** The now-playing panel's line: Record, or what is being recorded and
 *  Stop. */
export function PlayerRecordLine({ record }: { record: PlayerRecord }) {
  const dispatch = useDispatch();
  switch (record.kind) {
    case "record":
      return (
        <div className="watch-rec">
          <button
            className="ctl"
            data-tip={`Record “${record.programme.title}” (R)`}
            onClick={() => void recordFromPlayer(dispatch, record)}
          >
            <span className="rec-dot" aria-hidden="true" />
            Record
          </button>
        </div>
      );
    case "saving":
      return (
        <div className="watch-rec">
          <span className="watch-rec-state">
            <span className="rec-dot saving" aria-hidden="true" />
            Saving the recording…
          </span>
        </div>
      );
    case "recording":
    case "scheduled": {
      const recording = record.kind === "recording";
      return (
        <div className="watch-rec">
          <span className="watch-rec-state">
            <span className={`rec-dot ${recording ? "live" : ""}`} aria-hidden="true" />
            {recording ? "Recording" : "Will record"}
          </span>
          <button
            className="ctl"
            data-tip={
              recording
                ? `Stop recording “${record.rec.title}”: what was recorded is kept`
                : `Cancel recording “${record.rec.title}”`
            }
            onClick={() => void cancelRecording(dispatch, record.rec)}
          >
            {recording ? "Stop" : "Cancel"}
          </button>
        </div>
      );
    }
  }
}
