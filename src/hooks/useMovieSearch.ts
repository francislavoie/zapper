// The Movies tab's search: the on-demand search (useVodSearch) over the
// service's movie index.

import { backend } from "../backend";
import type { Async } from "../logic/async";
import type { MovieDto } from "../types";
import { useVodSearch } from "./useVodSearch";

export function useMovieSearch(query: string): Async<MovieDto[]> {
  return useVodSearch(query, backend.searchMovies);
}
