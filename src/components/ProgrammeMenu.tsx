// The context menu over a programme, in the guide and in the programme
// search results: record it, or cancel its recording.
//
// Recording is the recording service's job (zapper-server, Settings ›
// Recording); when it is not running the menu says so rather than offering
// something that cannot happen. Before a programme is recorded, the service
// is asked what it would clash with on an account with too few connections
// (docs/recording.md, "Sharing connections"), and the menu says so.

import { useEffect, useState } from "react";
import { cancelRecording, recordProgramme } from "../actions";
import { backend } from "../backend";
import { useClock } from "../hooks/useClock";
import {
  canRecord,
  conflictLosesProgramme,
  conflictText,
  episodeLabel,
  markKey,
  marks,
  scheduleRequest,
} from "../logic/recordings";
import { useApp, useDispatch } from "../store";
import type { ConflictDto } from "../types";
import { ContextMenu } from "./ContextMenu";

export function ProgrammeMenu() {
  const { programmeMenu: menu, recordings } = useApp();
  const dispatch = useDispatch();
  const now = useClock(15_000);
  const [conflicts, setConflicts] = useState<{ key: string; list: ConflictDto[] } | null>(null);

  const rec =
    menu && recordings
      ? marks(recordings).get(markKey(menu.channel.streamId, menu.programme.start))
      : undefined;
  const running = recordings !== null;
  const key = menu ? markKey(menu.channel.streamId, menu.programme.start) : null;
  const ask = menu !== null && running && !rec && canRecord(menu.programme, now);

  useEffect(() => {
    if (!ask || !menu || !key) return;
    let current = true;
    backend
      .previewRecording(scheduleRequest(menu.channel, menu.programme))
      .then((list) => {
        if (current) setConflicts({ key, list });
      })
      // Nothing to say is better than an error over a menu that works.
      .catch(() => undefined);
    return () => {
      current = false;
    };
    // The menu's programme is what matters; `menu` itself changes identity
    // with every open.
  }, [ask, key]);

  if (!menu) return null;

  const close = () => dispatch({ type: "programmeMenu", menu: null });
  const { channel, programme } = menu;
  // A programme already scheduled has its conflicts from the list.
  const clashes = rec ? (rec.conflicts ?? []) : ask && conflicts?.key === key ? conflicts.list : [];

  return (
    <ContextMenu
      x={menu.x}
      y={menu.y}
      heading={episodeLabel(programme) ? `${programme.title} · ${episodeLabel(programme)}` : programme.title}
      onClose={close}
      takeFocus={menu.keyboard}
    >
      {rec ? (
        <button
          className="tb-menu-item"
          role="menuitem"
          onClick={() => void cancelRecording(dispatch, rec)}
        >
          <span className="tb-menu-label" data-tip-overflow="">
            {rec.state === "recording" ? "Stop recording" : "Cancel recording"}
          </span>
        </button>
      ) : (
        <button
          className="tb-menu-item"
          role="menuitem"
          disabled={!running || !canRecord(programme, now)}
          onClick={() => void recordProgramme(dispatch, channel, programme)}
        >
          <span className="tb-menu-label" data-tip-overflow="">
            <span className="rec-dot" aria-hidden="true" /> Record
          </span>
        </button>
      )}
      <button
        className="tb-menu-item"
        role="menuitem"
        onClick={() => dispatch({ type: "recordingsDialog", open: true })}
      >
        <span className="tb-menu-label" data-tip-overflow="">
          Recordings…
        </span>
      </button>
      <div className="tb-menu-foot">
        {!running ? (
          "The recording service is not running: Settings › Recording."
        ) : !rec && !canRecord(programme, now) ? (
          "This programme is over."
        ) : rec?.state === "recording" ? (
          "Stopping keeps what has been recorded."
        ) : clashes.length > 0 ? (
          clashes.map((c, i) => (
            <div key={i} className={`menu-conflict ${conflictLosesProgramme(c) ? "bad" : ""}`}>
              {conflictText(c)}
            </div>
          ))
        ) : (
          "A recording uses one of the account’s connections while it runs, shared with any other recording of the same channel, and waits for one if none is free."
        )}
      </div>
    </ContextMenu>
  );
}
