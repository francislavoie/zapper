// Sync is background work, so it gets a hairline at the top of the window and
// nothing more. A modal here would block browsing over data that is already
// perfectly serviceable: stale data keeps serving while a sync runs.

import { backend } from "../backend";
import { run, useApp, useDispatch } from "../store";
import type { SyncPhase } from "../types";

const PHASE_LABEL: Record<SyncPhase, string> = {
  idle: "",
  categories: "categories",
  channels: "channels",
  epg: "guide",
  failed: "sync failed",
};

export function SyncStrip() {
  const { sync } = useApp();
  const dispatch = useDispatch();
  const busy =
    sync.phase === "categories" ||
    sync.phase === "channels" ||
    sync.phase === "epg";

  if (!busy && sync.phase !== "failed") return null;

  return (
    <div className={`sync-strip ${sync.phase === "failed" ? "bad" : ""}`}>
      <div className="sync-bar" aria-hidden />
      <div className="sync-text">
        <span className="sync-phase">{PHASE_LABEL[sync.phase]}</span>
        {sync.message ? <span className="sync-msg">{sync.message}</span> : null}
        {sync.phase === "failed" ? (
          <button
            className="link"
            onClick={() => void run(dispatch, "sync", backend.syncNow())}
          >
            retry
          </button>
        ) : null}
      </div>
    </div>
  );
}
