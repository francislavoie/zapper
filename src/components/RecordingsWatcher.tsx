// Keeps the store's list of recordings current, for the guide's dots, the
// programme menu, the toolbar's indicator and the Recordings dialog. Draws
// nothing.
//
// How often is `pollDelay`'s: every two seconds while anything records or
// is being saved, and for a few seconds after something is asked of the
// service; every few seconds while the dialog is open; otherwise every half
// minute. A service that is not running answers nothing, which is cheap to
// ask about: the request goes to 127.0.0.1 and fails at once.

import { useEffect, useState } from "react";
import { refreshRecordings } from "../actions";
import { pollDelay } from "../logic/recordings";
import { useApp, useDispatch } from "../store";

export function RecordingsWatcher() {
  const { recordings, recordingsOpen, recordingsTouchedAt } = useApp();
  const dispatch = useDispatch();
  // Bumped after each fetch, so the next is scheduled from what it found —
  // even when it found nothing, as from a service that is not running.
  const [fetched, setFetched] = useState(0);
  useEffect(() => {
    // The first look is at once; an action asked of the service brings the
    // next one forward too, since its delay is then the short one.
    const delay =
      fetched === 0 ? 0 : pollDelay(recordings, recordingsOpen, Date.now() - recordingsTouchedAt);
    let current = true;
    const id = setTimeout(() => {
      void refreshRecordings(dispatch).finally(() => {
        if (current) setFetched((n) => n + 1);
      });
    }, delay);
    return () => {
      current = false;
      clearTimeout(id);
    };
    // `recordings` is read for the delay but deliberately not a dependency:
    // each fetch replaces it, and `fetched` already schedules the next.
  }, [dispatch, fetched, recordingsOpen, recordingsTouchedAt]);
  return null;
}
