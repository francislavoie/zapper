// The band above the Recordings list: the selected recording, and what can be
// done to it. Everything the old Recordings window offered per row is here:
// stop or cancel it, open its folder, delete it, change its backup, read its
// conflicts and notes.

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
  recordingFacts,
  recordingRowStatus,
  recordingStatus,
  recordingWhen,
} from "../logic/recordings";
import { categoryNames } from "../logic/searchGroups";
import { useApp, useDispatch } from "../store";
import type { BackupsDto, ChannelDto, RecordingDto } from "../types";
import { RecDot } from "./RecDot";

/** The channel a recording falls back to when its own feed fails, and a
 *  small picker to change it: the service's candidates, best first, and
 *  "No backup". Any other channel can be chosen from the guide's menu. */
export function BackupLine({ rec }: { rec: RecordingDto }) {
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

/** The selected recording: what it is, how it is going, and what can be done
 *  with it. Keyed by the recording's id where it is used, so a half-made
 *  choice (the delete confirmation, the open notes) does not follow the
 *  selection to another recording. */
export function RecordingBand({ rec, channel }: { rec: RecordingDto; channel: ChannelDto | null }) {
  const dispatch = useDispatch();
  const { categories } = useApp();
  const now = useClock(5_000);
  const [confirming, setConfirming] = useState(false);
  const [notes, setNotes] = useState(false);
  const label = episodeLabel(rec);
  const category =
    channel?.categoryId != null && categories.status === "ready"
      ? categoryNames(categories.data).get(channel.categoryId)
      : undefined;
  const barred = rec.state === "recording" || rec.state === "finishing";
  const active = barred || rec.state === "scheduled";
  const status = recordingStatus(rec, now);
  const statusText = rec.state === "recording" && !rec.waiting ? `recording · ${status}` : status;
  const openFolder = () =>
    backend
      .openRecordingFolder(rec.id)
      .catch((e: unknown) => dispatch({ type: "toast", text: `Open folder: ${errorMessage(e)}` }));

  return (
    <>
      <div className="watch-body">
      <h2 className="watch-title-h" data-tip-overflow="">
        {active ? <RecDot recording={rec} /> : null}
        {rec.title}
        {label ? ` ${label}` : ""}
      </h2>
      <p className="watch-cat" data-tip-overflow="">
        {[rec.channelName, category].filter(Boolean).join(" · ")}
      </p>
      <div className="watch-sub">
        <span className="watch-time">{recordingWhen(rec, now)}</span>
      </div>
      {barred ? (
        <div className="watch-prog-line">
          <div className="watch-progress">
            <span
              style={{
                width: `${Math.round((rec.state === "recording" ? (recordingRowStatus(rec, now).progress ?? 0) : 1) * 100)}%`,
              }}
            />
          </div>
          <span className={`watch-left ${rec.waiting ? "bad" : ""}`}>{statusText}</span>
        </div>
      ) : (
        <p className={`watch-state ${rec.state === "failed" ? "bad" : ""}`}>{status}</p>
      )}
      <p className="watch-facts">
        {recordingFacts(rec)}
      </p>
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
      <div className="watch-actions">
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
        {capabilities.localService && (barred || rec.state === "done") ? (
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
    </>
  );
}
