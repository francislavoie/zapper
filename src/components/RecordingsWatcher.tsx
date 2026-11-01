// Keeps the store's list of recordings current, for the guide's red dots,
// the programme menu and the Recordings dialog. Draws nothing.
//
// Every half minute, and every few seconds while the dialog is open (sizes
// grow, recordings start and finish). A service that is not running answers
// nothing, which is cheap to ask about: the request goes to 127.0.0.1 and
// fails at once.

import { useEffect } from "react";
import { refreshRecordings } from "../actions";
import { useApp, useDispatch } from "../store";

const IDLE_MS = 30_000;
const OPEN_MS = 4_000;

export function RecordingsWatcher() {
  const { recordingsOpen } = useApp();
  const dispatch = useDispatch();
  useEffect(() => {
    void refreshRecordings(dispatch);
    const id = setInterval(() => void refreshRecordings(dispatch), recordingsOpen ? OPEN_MS : IDLE_MS);
    return () => clearInterval(id);
  }, [dispatch, recordingsOpen]);
  return null;
}
