// The band above the Recordings list: the selected recording, and what can be
// done to it. Its pieces arrive here as they are needed; BackupLine, the
// picker for a recording's backup channel, is the first.

import { useEffect, useState } from "react";
import { setRecordingBackup } from "../actions";
import { backend } from "../backend";
import { errorMessage } from "../logic/async";
import { backupCandidateLabel, backupText } from "../logic/recordings";
import { useDispatch } from "../store";
import type { BackupsDto, RecordingDto } from "../types";

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
