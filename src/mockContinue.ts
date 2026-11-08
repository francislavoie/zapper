// The mock's Continue watching: the movies' and the shows' places merged,
// newest first. Removing an entry hides it (in memory) until it plays again,
// as the catalogue's removal does. Every name is made up (see mockMovies.ts
// and mockShows.ts).

import { continueKey } from "./logic/continueWatching";
import type { ContinueItemDto, ContinueKind, MarksDto } from "./types";

interface Source {
  continueEntries(): ContinueItemDto[];
}

interface Marks {
  marksOf(id: number): MarksDto;
}

const delay = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

export function createMockContinue(movies: Source & Marks, shows: Source & Marks) {
  /** Entry key -> when it was removed, unix seconds. */
  const removed = new Map<string, number>();

  return {
    continueWatching: async (limit: number): Promise<ContinueItemDto[]> => {
      await delay(60);
      return [...movies.continueEntries(), ...shows.continueEntries()]
        .filter((i) => i.lastWatched > (removed.get(continueKey(i)) ?? -Infinity))
        .sort((a, b) => b.lastWatched - a.lastWatched)
        .slice(0, Math.max(0, limit));
    },

    removeFromContinue: async (kind: ContinueKind, id: number): Promise<void> => {
      await delay(40);
      removed.set(`${kind === "movie" ? "movie" : "show"}:${id}`, Math.floor(Date.now() / 1000));
    },

    movieMarks: async (movieId: number): Promise<MarksDto> => {
      await delay(30);
      return movies.marksOf(movieId);
    },

    episodeMarks: async (episodeId: number): Promise<MarksDto> => {
      await delay(30);
      return shows.marksOf(episodeId);
    },
  };
}
