// Recordings, from the menu or the palette: what the recording service has
// scheduled, is recording, has recorded, and could not record. Minimal by
// design — a list with the few things worth doing to each row; the files
// themselves are for a media library (Jellyfin and the like) to play.

import { useState } from "react";
import { cancelRecording, deleteRecording } from "../actions";
import { backend } from "../backend";
import { useClock } from "../hooks/useClock";
import { errorMessage } from "../logic/async";
import { groupRecordings, recordingStatus, recordingWhen } from "../logic/recordings";
import { useApp, useDispatch } from "../store";
import type { RecordingDto } from "../types";
import { RecDot } from "./RecDot";

export function RecordingsModal() {
  const { recordings, recordingsError } = useApp();
  const dispatch = useDispatch();
  const now = useClock(5_000);
  const close = () => dispatch({ type: "recordingsDialog", open: false });
  const groups = recordings ? groupRecordings(recordings) : [];

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) close();
      }}
    >
      <div className="about recordings" role="dialog" aria-label="Recordings">
        <h2>Recordings</h2>
        {recordings === null ? (
          <p className="pane-note">
            {recordingsError ?? "The recording service is not running."} Install or start it in
            Settings › Recording.
          </p>
        ) : groups.length === 0 ? (
          <p className="pane-note">
            Nothing recorded or scheduled. Right-click a programme in the guide, or in the
            programme search results, to record it.
          </p>
        ) : (
          <div className="rec-list">
            {groups.map((g) => (
              <section key={g.state} className="rec-group">
                <h4>{g.label}</h4>
                {g.items.map((r) => (
                  <RecordingRow key={r.id} rec={r} now={now} />
                ))}
              </section>
            ))}
          </div>
        )}
        <div className="about-actions">
          <button className="ctl" onClick={close} autoFocus>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}

function RecordingRow({ rec, now }: { rec: RecordingDto; now: number }) {
  const dispatch = useDispatch();
  const [confirming, setConfirming] = useState(false);
  const [notes, setNotes] = useState(false);
  const episode =
    rec.season != null && rec.episode != null
      ? ` S${String(rec.season).padStart(2, "0")}E${String(rec.episode).padStart(2, "0")}`
      : "";
  const openFolder = () =>
    backend
      .openRecordingFolder(rec.id)
      .catch((e: unknown) => dispatch({ type: "toast", text: `Open folder: ${errorMessage(e)}` }));

  return (
    <div className={`rec-row ${rec.state}`}>
      <div className="rec-main">
        <div className="rec-title" data-tip-overflow="">
          {rec.state === "recording" || rec.state === "scheduled" ? <RecDot recording={rec} /> : null}
          {rec.title}
          {episode}
        </div>
        <div className="rec-sub">
          <span>{recordingWhen(rec, now)}</span>
          <span className="rec-chan" data-tip-overflow="">
            {rec.channelName}
          </span>
          <span className={`rec-status ${rec.state === "failed" || rec.waiting ? "bad" : ""}`}>
            {recordingStatus(rec, now)}
          </span>
        </div>
        {rec.notes.length > 0 ? (
          <button className="link rec-notes-toggle" onClick={() => setNotes(!notes)}>
            {notes ? "Hide notes" : `${rec.notes.length} ${rec.notes.length === 1 ? "note" : "notes"}`}
          </button>
        ) : null}
        {notes ? (
          <ul className="rec-notes">
            {rec.notes.map((n, i) => (
              <li key={i}>{n}</li>
            ))}
          </ul>
        ) : null}
      </div>
      <div className="rec-actions">
        {rec.state === "scheduled" ? (
          <button className="ctl" onClick={() => void cancelRecording(dispatch, rec)}>
            Cancel
          </button>
        ) : null}
        {rec.state === "recording" ? (
          <button className="ctl" onClick={() => void cancelRecording(dispatch, rec)}>
            Stop
          </button>
        ) : null}
        {rec.state === "recording" || rec.state === "done" ? (
          <button className="ctl" onClick={() => void openFolder()}>
            Open folder
          </button>
        ) : null}
        {rec.state === "done" || rec.state === "failed" ? (
          confirming ? (
            <button
              className="ctl danger"
              onClick={() => void deleteRecording(dispatch, rec)}
              onBlur={() => setConfirming(false)}
              autoFocus
            >
              {rec.parts.length > 0 ? "Delete the files" : "Remove"}
            </button>
          ) : (
            <button className="ctl" onClick={() => setConfirming(true)}>
              {rec.state === "done" ? "Delete…" : "Remove"}
            </button>
          )
        ) : null}
      </div>
    </div>
  );
}
