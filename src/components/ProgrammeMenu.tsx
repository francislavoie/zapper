// The context menu over a programme, in the guide and in the programme
// search results: record it, or cancel its recording.
//
// Recording is the recording service's job (zapper-server, Settings ›
// Recording); when it is not running the menu says so rather than offering
// something that cannot happen.

import { cancelRecording, recordProgramme } from "../actions";
import { useClock } from "../hooks/useClock";
import { canRecord, markKey, marks } from "../logic/recordings";
import { useApp, useDispatch } from "../store";
import { ContextMenu } from "./ContextMenu";

export function ProgrammeMenu() {
  const { programmeMenu: menu, recordings } = useApp();
  const dispatch = useDispatch();
  const now = useClock(15_000);
  if (!menu) return null;

  const close = () => dispatch({ type: "programmeMenu", menu: null });
  const { channel, programme } = menu;
  const rec = recordings ? marks(recordings).get(markKey(channel.streamId, programme.start)) : undefined;
  const running = recordings !== null;

  return (
    <ContextMenu
      x={menu.x}
      y={menu.y}
      heading={programme.title}
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
        {!running
          ? "The recording service is not running: Settings › Recording."
          : !rec && !canRecord(programme, now)
            ? "This programme is over."
            : rec?.state === "recording"
              ? "Stopping keeps what has been recorded."
              : "A recording uses one of the account’s connections while it runs, and waits for one if none is free."}
      </div>
    </ContextMenu>
  );
}
