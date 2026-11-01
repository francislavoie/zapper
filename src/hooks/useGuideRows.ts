// Programmes for the rows the guide can actually see.
//
// The catalogue can be tens of thousands of channels. `guide_rows` is called with the visible
// window's stream ids and nothing else, coalesced so that a flick of the
// scroll wheel is one request rather than forty.
//
// Time is asked about in fixed six-hour chunks (GUIDE_CHUNK), not for the
// window on screen. The guide scrolls sideways by the pixel, and a cache
// keyed on the exact window would be emptied and refetched every frame of a
// swipe. Keyed by chunk, a swipe asks nothing new until it nears the next
// chunk — and the window's half-screen margin either side means it usually
// asks before it gets there. Chunks well away from the window are dropped,
// so a long session of scrolling about does not grow without bound.

import { useEffect, useReducer, useRef } from "react";
import { backend } from "../backend";
import { errorMessage } from "../logic/async";
import { GUIDE_CHUNK, chunksFor, visibleProgrammes, type TimeWindow } from "../logic/guide";
import type { ProgrammeDto } from "../types";

/** Long enough to swallow a scroll, short enough not to feel like a stall. */
const COALESCE_MS = 90;

export interface GuideData {
  /** Only what overlaps the window, once each, in order. `undefined` means
   *  "not fetched yet" — which is not the same as a channel with no EPG, and
   *  the grid draws the two differently. */
  programmesFor: (streamId: number) => ProgrammeDto[] | undefined;
  error: string | null;
  loading: boolean;
  retry: () => void;
}

const key = (chunk: number, id: number) => `${chunk}:${id}`;

/** `epoch` empties the cache when it changes: pass something that changes
 *  when the guide data underneath does (the last sync), since chunks are
 *  otherwise kept for as long as the guide stays near them.
 *
 *  `win` is what is drawn and `target` is where the guide is headed: the grid
 *  eases between the two, and fetching for the destination rather than for
 *  every window passed through on the way means the data is asked for as soon
 *  as the move starts, once, instead of chunk by chunk as an animation
 *  crosses them. */
export function useGuideRows(
  streamIds: number[],
  win: TimeWindow,
  epoch: number | null = null,
  target: TimeWindow = win,
): GuideData {
  const [, bump] = useReducer((n: number) => n + 1, 0);
  const [attempt, retryNow] = useReducer((n: number) => n + 1, 0);
  /** chunk start → stream id → that chunk's programmes for it. */
  const chunks = useRef(new Map<number, Map<number, ProgrammeDto[]>>());
  const inflight = useRef(new Set<string>());
  const failed = useRef(new Set<string>());
  const error = useRef<string | null>(null);
  /** Bumped on every reset, so an answer to a question from before it is
   *  dropped rather than written into the new cache. */
  const generation = useRef(0);

  const epochRef = useRef(epoch);
  if (epochRef.current !== epoch) {
    epochRef.current = epoch;
    chunks.current = new Map();
    inflight.current = new Set();
    failed.current = new Set();
    error.current = null;
    generation.current += 1;
  }

  const span = target.to - target.from;
  // What has to be there to draw the window, and what is worth having ready.
  const visible = chunksFor(win);
  const wanted = chunksFor(target, span / 2);
  const wantedKey = wanted.join(",");
  const wantedRef = useRef(wanted);
  wantedRef.current = wanted;

  // The ids change identity every render; the effect keys off their contents.
  const idsKey = streamIds.join(",");
  const idsRef = useRef(streamIds);
  idsRef.current = streamIds;

  useEffect(() => {
    const timer = window.setTimeout(() => {
      const want = wantedRef.current;
      const mine = generation.current;

      // Forget chunks more than one beyond what is wanted. One, not zero, so
      // a swipe back and forth across a boundary does not refetch each time.
      const lo = want[0] - GUIDE_CHUNK;
      const hi = want[want.length - 1] + GUIDE_CHUNK;
      for (const c of [...chunks.current.keys()]) {
        if (c < lo || c > hi) chunks.current.delete(c);
      }
      for (const k of [...failed.current]) {
        const c = Number(k.slice(0, k.indexOf(":")));
        if (c < lo || c > hi) failed.current.delete(k);
      }

      for (const chunk of want) {
        const have = chunks.current.get(chunk);
        const ids = idsRef.current.filter(
          (id) =>
            !have?.has(id) &&
            !inflight.current.has(key(chunk, id)) &&
            !failed.current.has(key(chunk, id)),
        );
        if (ids.length === 0) continue;
        for (const id of ids) inflight.current.add(key(chunk, id));
        backend
          .guideRows(ids, chunk, chunk + GUIDE_CHUNK)
          .then((got) => {
            if (generation.current !== mine) return;
            const byId = new Map(got.map((r) => [r.streamId, r.programmes]));
            let rows = chunks.current.get(chunk);
            if (!rows) chunks.current.set(chunk, (rows = new Map()));
            for (const id of ids) {
              inflight.current.delete(key(chunk, id));
              // A row the backend did not return has no guide data, which is
              // the common case and gets recorded as such rather than retried.
              rows.set(id, byId.get(id) ?? []);
            }
            error.current = null;
            bump();
          })
          .catch((e: unknown) => {
            if (generation.current !== mine) return;
            for (const id of ids) {
              inflight.current.delete(key(chunk, id));
              // Remember the failure: scrolling must not hammer a dead command.
              failed.current.add(key(chunk, id));
            }
            error.current = errorMessage(e);
            bump();
          });
      }
    }, COALESCE_MS);
    return () => window.clearTimeout(timer);
  }, [idsKey, wantedKey, epoch, attempt]);

  return {
    programmesFor: (id) => {
      const lists: ProgrammeDto[][] = [];
      let missing = false;
      for (const c of visible) {
        const list = chunks.current.get(c)?.get(id);
        if (list) lists.push(list);
        else missing = true;
      }
      if (lists.length === 0) return undefined;
      const out = visibleProgrammes(lists, win);
      // Half a window loaded and nothing in that half is still loading, not
      // a channel without a guide.
      return out.length === 0 && missing ? undefined : out;
    },
    error: error.current,
    loading: inflight.current.size > 0,
    retry: () => {
      failed.current = new Set();
      error.current = null;
      // The ids and chunks have not changed, so the effect needs its own
      // reason to run again.
      retryNow();
    },
  };
}
