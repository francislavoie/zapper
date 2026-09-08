// Programmes for the rows the guide can actually see.
//
// The catalogue can be tens of thousands of channels. `guide_rows` is called with the visible
// window's stream ids and nothing else, coalesced so that a flick of the
// scroll wheel is one request rather than forty, and cached per time window —
// paging the guide sideways is a different question and gets asked afresh.

import { useEffect, useReducer, useRef } from "react";
import { backend } from "../backend";
import { errorMessage } from "../logic/async";
import type { TimeWindow } from "../logic/guide";
import type { ProgrammeDto } from "../types";

/** Long enough to swallow a scroll, short enough not to feel like a stall. */
const COALESCE_MS = 90;

export interface GuideData {
  /** `undefined` means "not fetched yet" — which is not the same as a channel
   *  with no EPG, and the grid draws the two differently. */
  programmesFor: (streamId: number) => ProgrammeDto[] | undefined;
  error: string | null;
  loading: boolean;
  retry: () => void;
}

export function useGuideRows(streamIds: number[], win: TimeWindow): GuideData {
  const [, bump] = useReducer((n: number) => n + 1, 0);
  const rows = useRef(new Map<number, ProgrammeDto[]>());
  const inflight = useRef(new Set<number>());
  const failed = useRef(new Set<number>());
  const error = useRef<string | null>(null);

  // A new time window is a new question: nothing already fetched answers it.
  const windowKey = `${win.from}:${win.to}`;
  const keyRef = useRef(windowKey);
  if (keyRef.current !== windowKey) {
    keyRef.current = windowKey;
    rows.current = new Map();
    inflight.current = new Set();
    failed.current = new Set();
    error.current = null;
  }

  // The ids change identity every render; the effect keys off their contents.
  const idsKey = streamIds.join(",");
  const idsRef = useRef(streamIds);
  idsRef.current = streamIds;

  useEffect(() => {
    const timer = window.setTimeout(() => {
      const want = idsRef.current.filter(
        (id) =>
          !rows.current.has(id) && !inflight.current.has(id) && !failed.current.has(id),
      );
      if (want.length === 0) return;
      const mine = keyRef.current;
      for (const id of want) inflight.current.add(id);
      backend
        .guideRows(want, win.from, win.to)
        .then((got) => {
          if (keyRef.current !== mine) return;
          const byId = new Map(got.map((r) => [r.streamId, r.programmes]));
          for (const id of want) {
            inflight.current.delete(id);
            // A row the backend did not return has no guide data, which is the
            // common case and gets recorded as such rather than retried.
            rows.current.set(id, byId.get(id) ?? []);
          }
          error.current = null;
          bump();
        })
        .catch((e: unknown) => {
          if (keyRef.current !== mine) return;
          for (const id of want) {
            inflight.current.delete(id);
            // Remember the failure: scrolling must not hammer a dead command.
            failed.current.add(id);
          }
          error.current = errorMessage(e);
          bump();
        });
    }, COALESCE_MS);
    return () => window.clearTimeout(timer);
  }, [idsKey, win.from, win.to]);

  return {
    programmesFor: (id) => rows.current.get(id),
    error: error.current,
    loading: inflight.current.size > 0,
    retry: () => {
      failed.current = new Set();
      error.current = null;
      bump();
    },
  };
}
