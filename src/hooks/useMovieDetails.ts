// The details of the selected movie: its plot, genres and people, which the
// list does not carry. Fetched from the provider one title at a time, so a
// held arrow key must not ask about every row it crosses: the request waits
// until the selection has been still for `delayMs`, and a newer selection
// drops an older answer. Answers are kept (200 titles, oldest forgotten
// first) so going back to a title is instant.

import { useEffect, useState } from "react";
import { backend } from "../backend";
import { errorMessage, failed, idle, loading, ready, type Async } from "../logic/async";
import { recallDetails, rememberDetails } from "../logic/movies";
import type { MovieDetailsDto, MovieDto } from "../types";

const KEPT = 200;
const cache = new Map<number, MovieDetailsDto>();

export function useMovieDetails(movie: MovieDto | null, delayMs: number): Async<MovieDetailsDto> {
  const id = movie?.movieId ?? null;
  const [result, setResult] = useState<{ id: number; value: Async<MovieDetailsDto> } | null>(null);

  useEffect(() => {
    if (id === null) return;
    if (recallDetails(cache, id) !== undefined) return;
    // Whatever an earlier visit to this title said is not an answer yet.
    setResult(null);
    let cancelled = false;
    const timer = window.setTimeout(() => {
      setResult({ id, value: loading() });
      backend
        .movieDetails(id)
        .then((d) => {
          rememberDetails(cache, id, d, KEPT);
          if (!cancelled) setResult({ id, value: ready(d) });
        })
        .catch((e: unknown) => {
          if (!cancelled) setResult({ id, value: failed(errorMessage(e)) });
        });
    }, delayMs);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [id, delayMs]);

  if (id === null) return idle();
  const known = recallDetails(cache, id);
  if (known !== undefined) return ready(known);
  // An answer for an earlier selection is not this one's.
  return result?.id === id ? result.value : loading();
}
