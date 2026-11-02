// Recordings, from the menu or the palette: what the recording service has
// scheduled, is recording, has recorded, and could not record. Minimal by
// design — a list with the few things worth doing to each row; the files
// themselves are for a media library (Jellyfin and the like) to play.

import { useEffect, useState } from "react";
import { cancelRecording, deleteRecording, setRecordingBackup } from "../actions";
import { backend, capabilities } from "../backend";
import { useClock } from "../hooks/useClock";
import { errorMessage } from "../logic/async";
import {
  backupCandidateLabel,
  backupText,
  conflictLosesProgramme,
  conflictText,
  episodeLabel,
  groupRecordings,
  recordingStatus,
  recordingWhen,
} from "../logic/recordings";
import { useApp, useDispatch } from "../store";
import type { BackupsDto, RecordingDto } from "../types";
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

/** The channel a recording falls back to when its own feed fails, and a
 *  small picker to change it: the service's candidates, best first, and
 *  "No backup". Any other channel can be chosen from the guide's menu. */
function BackupLine({ rec }: { rec: RecordingDto }) {
  const dispatch = useDispatch();
  const [open, setOpen] = useState(false);
  const [list, setList] = useState<BackupsDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    let current = true;
    setList(null);
    setError(null);
    backend
      .recordingBackups(rec.id)
      .then((l) => {
        if (current) setList(l);
      })
      .catch((e: unknown) => {
        if (current) setError(errorMessage(e));
      });
    return () => {
      current = false;
    };
  }, [open, rec.id]);

  const choose = async (streamId: number | null) => {
    setBusy(true);
    await setRecordingBackup(dispatch, rec, streamId);
    setBusy(false);
    setOpen(false);
  };

  return (
    <div className="rec-backup">
      <div className="rec-backup-now">
        <span className="rec-backup-name" data-tip-overflow="">
          {backupText(rec)}
        </span>
        <button className="link" aria-expanded={open} onClick={() => setOpen(!open)}>
          {open ? "Close" : "Change…"}
        </button>
      </div>
      {open ? (
        <div className="rec-backup-pick" role="listbox" aria-label={`Backup channel for ${rec.title}`}>
          {error ? (
            <div className="pane-note bad">{error}</div>
          ) : list === null ? (
            <div className="pane-note">Looking for channels…</div>
          ) : (
            <>
              {list.candidates.length === 0 ? (
                <div className="pane-note">
                  No other channel carries this programme. To use one anyway, right-click a
                  programme on it at this time in the guide.
                </div>
              ) : null}
              {list.candidates.map((b) => (
                <button
                  key={b.streamId}
                  role="option"
                  aria-selected={b.streamId === rec.backupStreamId}
                  className={`rec-backup-option ${b.streamId === rec.backupStreamId ? "on" : ""}`}
                  disabled={busy}
                  onClick={() => void choose(b.streamId)}
                >
                  <span data-tip-overflow="">{backupCandidateLabel(b)}</span>
                </button>
              ))}
              {rec.backupStreamId != null ? (
                <button
                  role="option"
                  aria-selected={false}
                  className="rec-backup-option"
                  disabled={busy}
                  onClick={() => void choose(null)}
                >
                  <span>No backup</span>
                </button>
              ) : null}
            </>
          )}
        </div>
      ) : null}
    </div>
  );
}

function RecordingRow({ rec, now }: { rec: RecordingDto; now: number }) {
  const dispatch = useDispatch();
  const [confirming, setConfirming] = useState(false);
  const [notes, setNotes] = useState(false);
  const label = episodeLabel(rec);
  const episode = label ? ` ${label}` : "";
  const openFolder = () =>
    backend
      .openRecordingFolder(rec.id)
      .catch((e: unknown) => dispatch({ type: "toast", text: `Open folder: ${errorMessage(e)}` }));

  return (
    <div className={`rec-row ${rec.state}`}>
      <div className="rec-main">
        <div className="rec-title" data-tip-overflow="">
          {rec.state === "recording" || rec.state === "scheduled" || rec.state === "finishing" ? (
            <RecDot recording={rec} />
          ) : null}
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
        {rec.state === "scheduled" || rec.state === "recording" ? <BackupLine rec={rec} /> : null}
        {rec.conflicts?.length ? (
          <ul className="rec-conflicts">
            {rec.conflicts.map((c, i) => (
              <li key={i} className={`rec-conflict ${conflictLosesProgramme(c) ? "bad" : ""}`}>
                {conflictText(c)}
              </li>
            ))}
          </ul>
        ) : null}
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
        {capabilities.localService &&
        (rec.state === "recording" || rec.state === "finishing" || rec.state === "done") ? (
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
