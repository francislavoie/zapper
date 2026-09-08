// Now and next for the channel that is playing.
//
// `ChannelDto.nowPlaying` is one programme and nothing more, so "next" needs a
// round trip. `guide_rows` is the cheapest one available — a single row — and
// it is allowed to fail: most channels have no EPG at all, and a request can
// fail like any other. Either way the panel falls back to what the
// catalogue already told us and says "no guide data" rather than an error.

import { useEffect, useState } from "react";
import { backend } from "../backend";
import { HOUR, splitNowNext, type NowNext } from "../logic/guide";
import type { ChannelDto, ProgrammeDto } from "../types";

/** An hour back, so a programme already in progress is included. */
const LOOKBACK = HOUR;
const LOOKAHEAD = 8 * HOUR;

export function useNowNext(channel: ChannelDto | null, nowSeconds: number): NowNext {
  const streamId = channel?.streamId ?? null;
  const [programmes, setProgrammes] = useState<ProgrammeDto[]>([]);

  useEffect(() => {
    setProgrammes([]);
    if (streamId === null) return;
    let live = true;
    const from = Math.floor(Date.now() / 1000) - LOOKBACK;
    backend
      .guideRows([streamId], from, from + LOOKAHEAD)
      .then((rows) => {
        if (!live) return;
        setProgrammes(rows.find((r) => r.streamId === streamId)?.programmes ?? []);
      })
      .catch(() => {
        // Silent on purpose. A missing guide is the normal case and a missing
        // command is the backend's problem, not something to shout about on
        // every channel change.
        if (live) setProgrammes([]);
      });
    return () => {
      live = false;
    };
  }, [streamId]);

  const split = splitNowNext(programmes, nowSeconds);
  return { now: split.now ?? channel?.nowPlaying ?? null, next: split.next };
}
