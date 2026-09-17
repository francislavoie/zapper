// A zoom change is otherwise silent: the interface simply becomes a different
// size, and "did that do anything, and how far am I from normal" is a fair
// question. This says the number for a moment and offers the way back.
//
// It does not appear at boot, only on a change — opening the app at the size
// you left it at is not news.

import { useEffect, useRef, useState } from "react";
import { stepZoom } from "../hooks/useZoom";
import { formatZoom, isDefaultZoom } from "../logic/zoom";
import { useApp, useDispatch } from "../store";

const SHOW_MS = 1600;

export function ZoomBadge() {
  const { zoom } = useApp();
  const dispatch = useDispatch();
  const [visible, setVisible] = useState(false);
  /** The zoom this has already reacted to. Seeded with the value at mount, so
   *  the remembered size is not announced as a change. */
  const shown = useRef(zoom);

  useEffect(() => {
    if (shown.current === zoom) return;
    shown.current = zoom;
    setVisible(true);
    const t = window.setTimeout(() => setVisible(false), SHOW_MS);
    return () => window.clearTimeout(t);
  }, [zoom]);

  if (!visible) return null;

  return (
    <div className="zoom-badge" role="status" aria-live="polite">
      <span className="zoom-level">{formatZoom(zoom)}</span>
      {isDefaultZoom(zoom) ? (
        <span className="zoom-hint">Ctrl + − 0</span>
      ) : (
        <button
          className="ctl ghost"
          title="Back to 100% (Ctrl+0)"
          onClick={() => stepZoom(dispatch, "reset")}
        >
          Reset
        </button>
      )}
    </div>
  );
}
