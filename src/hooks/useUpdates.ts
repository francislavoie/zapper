// Keeps the store's copy of the updater in step with the backend, and says so
// once when an update turns up on its own.
//
// Mounted once, at the shell. The backend pushes every state change; the
// first status is fetched as well, because a push sent before this listener
// existed is gone, and "not configured" or an update found by an earlier
// check would otherwise never reach the badge.

import { useEffect, useRef } from "react";
import { backend, subscribeUpdate } from "../backend";
import { announcement } from "../logic/updates";
import { useApp, useDispatch } from "../store";
import type { UpdateStatusDto } from "../types";

export function useUpdates() {
  const dispatch = useDispatch();
  /** The last version announced, or seen in the open dialog: each version is
   *  announced once. A ref, not the store: reading it must not re-run the
   *  subscription. */
  const announced = useRef<string | null>(null);
  /** With the dialog open the person is looking at the answer already; a
   *  toast saying the same thing would be noise. */
  const dialogOpen = useRef(false);
  dialogOpen.current = useApp().updateOpen;

  useEffect(() => {
    let alive = true;
    const take = (s: UpdateStatusDto) => {
      if (!alive) return;
      const text = announcement(s.state, announced.current);
      if (text) announced.current = s.state.state === "available" ? s.state.release.version : null;
      dispatch({ type: "update", value: s });
      // Info, not error: nothing went wrong, and it should not look as if it
      // had. The badge in the title bar stays after the toast has gone. Not
      // over the open dialog, but the version still counts as announced.
      if (text && !dialogOpen.current) dispatch({ type: "toast", text, kind: "info" });
    };
    const unsub = subscribeUpdate(take);
    backend
      .updateStatus()
      .then(take)
      .catch(() => {
        // An older backend without the updater, or the mock's broken mode.
        // Nothing to show is the right thing to show.
      });
    return () => {
      alive = false;
      void unsub.then((u) => u());
    };
  }, [dispatch]);
}
