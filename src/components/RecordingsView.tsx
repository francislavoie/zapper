// Recordings, as the Live TV list pane shows them when the rail's Recordings
// is chosen: what is recording now, what is scheduled, what has been
// recorded, in one list under the same head as the channel list's. Choosing
// a row selects it; the band above shows it and offers what to do with it.

import { useEffect, useState } from "react";
import { useClock } from "../hooks/useClock";
import { useDiskFree } from "../hooks/useDiskFree";
import { useRecordingChannels } from "../hooks/useRecordingChannels";
import {
  bytes,
  episodeLabel,
  recordingRowStatus,
  recordingSections,
  recordingWhen,
  visibleSelection,
  type RecordingsFilter,
} from "../logic/recordings";
import { categoryNames } from "../logic/searchGroups";
import { useApp, useDispatch } from "../store";
import type { RecordingDto } from "../types";
import { Logo } from "./Logo";
import { RailToggle } from "./Rail";
import { RecDot } from "./RecDot";

const FILTERS: ReadonlyArray<[RecordingsFilter, string]> = [
  ["all", "All"],
  ["scheduled", "Scheduled"],
  ["recorded", "Recorded"],
];

/** The state's mark at the row's left: a red dot recording, a grey one
 *  saving, a clock while scheduled, nothing once it is over. */
function Glyph({ rec }: { rec: RecordingDto }) {
  if (rec.state === "recording" || rec.state === "finishing") return <RecDot recording={rec} />;
  if (rec.state === "scheduled")
    return (
      <span className="rec-clock" aria-hidden="true">
        ◷
      </span>
    );
  return <span />;
}

export function RecordingsView() {
  const { recordings, recordingsError, selectedRecordingId, categories } = useApp();
  const dispatch = useDispatch();
  const now = useClock(5_000);
  const free = useDiskFree();
  const channels = useRecordingChannels(recordings);
  const [filter, setFilter] = useState<RecordingsFilter>("all");
  const names = categoryNames(categories.status === "ready" ? categories.data : []);
  const sections = recordings ? recordingSections(recordings, filter) : [];

  // A filter that hides the selected row, or a recording that is gone,
  // leaves nothing selected rather than something out of sight.
  const kept = visibleSelection(sections, selectedRecordingId);
  useEffect(() => {
    if (kept !== selectedRecordingId) dispatch({ type: "selectRecording", id: kept });
  }, [dispatch, kept, selectedRecordingId]);

  return (
    <main className="list-pane rec-pane">
      <div className="list-head">
        <RailToggle />
        <span className="rec-dot live" aria-hidden="true" />
        <span className="list-title">Recordings</span>
        {free !== null ? <span className="rec-free">{bytes(free)} free</span> : null}
        <span className="spring" />
        <div className="view-switch" role="tablist" aria-label="Show">
          {FILTERS.map(([id, label]) => (
            <button
              key={id}
              role="tab"
              aria-selected={filter === id}
              className={filter === id ? "on" : ""}
              onClick={() => setFilter(id)}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      <div className="list-cols rec-cols" aria-hidden>
        <span />
        <span />
        <span>Programme</span>
        <span>When</span>
        <span>Status</span>
      </div>

      <div className="rec-scroll">
        {recordings === null ? (
          <p className="pane-note">
            {recordingsError ?? "The recording service is not running."} Install or start it in
            Settings › Recording.
          </p>
        ) : recordings.length === 0 ? (
          <p className="pane-note">
            Nothing recorded or scheduled. Right-click a programme in the guide, or in the
            programme search results, to record it.
          </p>
        ) : (
          sections.map((s) => (
            <section key={s.id}>
              <h4 className="rec-group-head">{s.label}</h4>
              {s.items.map((r) => {
                const ch = channels.get(r.streamId);
                const category = ch?.categoryId ? names.get(ch.categoryId) : undefined;
                const status = recordingRowStatus(r, now);
                const label = episodeLabel(r);
                const sub = [r.channelName, category].filter(Boolean).join(" · ");
                return (
                  <button
                    key={r.id}
                    className={`rec-item ${r.id === selectedRecordingId ? "sel" : ""}`}
                    aria-pressed={r.id === selectedRecordingId}
                    onClick={() => dispatch({ type: "selectRecording", id: r.id })}
                  >
                    <Glyph rec={r} />
                    <Logo url={ch?.logoUrl ?? null} />
                    <span className="rec-text">
                      <span className="rec-title" data-tip-overflow="">
                        {r.title}
                        {label ? ` ${label}` : ""}
                      </span>
                      <span className="rec-sub" data-tip-overflow="">
                        {sub}
                      </span>
                    </span>
                    <span className="rec-when">{recordingWhen(r, now)}</span>
                    <span className={`rec-status ${status.tone}`}>
                      {status.progress !== null ? (
                        <span className="rec-progress" aria-hidden="true">
                          <span style={{ width: `${Math.round(status.progress * 100)}%` }} />
                        </span>
                      ) : null}
                      <span className="rec-status-text" data-tip-overflow="">
                        {status.text}
                      </span>
                    </span>
                  </button>
                );
              })}
            </section>
          ))
        )}
      </div>
    </main>
  );
}
