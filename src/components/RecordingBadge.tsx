// The title bar's sign that the recording service is recording: a red dot
// and "Recording" ("Recording 2" for two), just left of Settings. Its
// tooltip lists each one — channel, programme, how long so far and until
// when, and whether it waits for a connection, has a conflict ahead or is on
// its backup channel — and a click opens Recordings. Grey and "Saving" once
// all that is left is saving recordings into their files. On a narrow
// window the word goes and the dot stays.
//
// Nothing at all when nothing records, or no service answers: the list is
// the one RecordingsWatcher keeps, which is fetched often while anything
// records.

import type { MouseEvent as ReactMouseEvent } from "react";
import { useClock } from "../hooks/useClock";
import { recordingIndicator } from "../logic/recordings";
import { useApp, useDispatch } from "../store";

export function RecordingBadge({ onMouseDown }: { onMouseDown?: (e: ReactMouseEvent) => void }) {
  const { recordings } = useApp();
  const dispatch = useDispatch();
  const now = useClock(15_000);
  const ind = recordingIndicator(recordings, now);
  if (!ind) return null;
  return (
    <button
      className={`tb-rec ${ind.tone}`}
      onMouseDown={onMouseDown}
      onClick={() => dispatch({ type: "showRecordings" })}
      data-tip={ind.tip}
      aria-label={`${ind.label}: ${ind.tip}`}
    >
      <span className={`rec-dot ${ind.tone === "live" ? "live" : "saving"}`} aria-hidden="true" />
      <span className="tb-rec-label">{ind.label}</span>
    </button>
  );
}
