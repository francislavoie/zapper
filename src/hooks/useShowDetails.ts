// The details of a show: its plot, genres, people, seasons and episodes,
// which the list does not carry. As a movie's (useMovieDetails): asked for
// only once the selection has been still for `delayMs`, a newer selection
// drops an older answer, and answers are kept (100 shows, oldest forgotten
// first) so going back to a show is instant.
//
// A show's details also change under it: an episode marked, played or
// stopped moves its season's count and what is next. The store bumps the
// show's revision when that happens (`shows.detailsRevision`), and a kept
// answer from an older revision is asked for again at once — still shown
// meanwhile, so the sheet does not blank for a tick on every ✓. The service
// answers that from its own cache: no provider call while it is fresh.

import { useEffect, useState } from "react";
import { backend } from "../backend";
import { errorMessage, failed, idle, loading, ready, type Async } from "../logic/async";
import { recallDetails, rememberDetails } from "../logic/movies";
import type { SeriesDetailsDto, SeriesDto } from "../types";

const KEPT = 100;

interface Kept {
  revision: number;
  details: SeriesDetailsDto;
}

const cache = new Map<number, Kept>();

/** Forgets every show's details: the shows are another account's now. */
export function clearShowDetails(): void {
  cache.clear();
}

export function useShowDetails(
  show: SeriesDto | null,
  delayMs: number,
  revision: number,
): Async<SeriesDetailsDto> {
  const id = show?.seriesId ?? null;
  const [result, setResult] = useState<{
    id: number;
    revision: number;
    value: Async<SeriesDetailsDto>;
  } | null>(null);

  useEffect(() => {
    if (id === null) return;
    const kept = recallDetails(cache, id);
    if (kept?.revision === revision) return;
    // Something older is shown meanwhile, and is asked about again without
    // waiting: this is not an arrow key crossing the list.
    const wait = kept ? 0 : delayMs;
    if (!kept) setResult(null);
    let cancelled = false;
    const timer = window.setTimeout(() => {
      // Another view (the band, the sheet) may have got it while this one
      // waited.
      const got = recallDetails(cache, id);
      if (got?.revision === revision) {
        setResult({ id, revision, value: ready(got.details) });
        return;
      }
      setResult({ id, revision, value: loading() });
      backend
        .seriesDetails(id)
        .then((d) => {
          // An answer to an older question must not replace a newer one.
          const now = cache.get(id);
          if (!now || now.revision <= revision) {
            rememberDetails(cache, id, { revision, details: d }, KEPT);
          }
          if (!cancelled) setResult({ id, revision, value: ready(d) });
        })
        .catch((e: unknown) => {
          if (!cancelled) setResult({ id, revision, value: failed(errorMessage(e)) });
        });
    }, wait);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [id, delayMs, revision]);

  if (id === null) return idle();
  const known = cache.get(id);
  if (known?.revision === revision) return ready(known.details);
  const mine = result?.id === id && result.revision === revision ? result.value : null;
  if (mine?.status === "ready") return mine;
  // On its way again, or failed to come again: what was there still stands.
  if (known) return ready(known.details);
  return mine ?? loading();
}
