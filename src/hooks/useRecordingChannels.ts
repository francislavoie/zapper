// The channels the recordings were made from, for their logos and
// categories: the catalogue's rows for the distinct stream ids, fetched again
// only when that set changes (the list itself is replaced every few seconds).

import { useEffect, useMemo, useState } from "react";
import { backend } from "../backend";
import type { ChannelDto, RecordingDto } from "../types";

const NONE: ReadonlyMap<number, ChannelDto> = new Map();

export function useRecordingChannels(recs: readonly RecordingDto[] | null): ReadonlyMap<number, ChannelDto> {
  // A string so the effect's dependency is the set, not the array.
  const key = useMemo(
    () => [...new Set((recs ?? []).map((r) => r.streamId))].sort((a, b) => a - b).join(","),
    [recs],
  );
  const [channels, setChannels] = useState(NONE);
  useEffect(() => {
    if (key === "") {
      setChannels(NONE);
      return;
    }
    let current = true;
    backend
      .channelsByIds(key.split(",").map(Number))
      .then((rows) => {
        if (current) setChannels(new Map(rows.map((c) => [c.streamId, c])));
      })
      .catch(() => {
        // Rows without logos are still rows.
      });
    return () => {
      current = false;
    };
  }, [key]);
  return channels;
}
