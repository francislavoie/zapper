// Free space on the recording folder's disk, for the Recordings pane's head.
// Asked on mount and once a minute: it changes slowly, and the service is
// the only one who knows.

import { useEffect, useState } from "react";
import { backend } from "../backend";

export function useDiskFree(): number | null {
  const [free, setFree] = useState<number | null>(null);
  useEffect(() => {
    let current = true;
    const ask = () =>
      backend
        .recordingStatus()
        .then((s) => {
          if (current) setFree(s.diskFree);
        })
        .catch(() => {
          // No service answering: there is nothing to say about its disk.
          if (current) setFree(null);
        });
    void ask();
    const id = setInterval(() => void ask(), 60_000);
    return () => {
      current = false;
      clearInterval(id);
    };
  }, []);
  return free;
}
