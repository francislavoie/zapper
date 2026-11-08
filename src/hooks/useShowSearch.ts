// The Shows tab's search: the on-demand search (useVodSearch) over the
// service's show index.

import { backend } from "../backend";
import type { Async } from "../logic/async";
import type { SeriesDto } from "../types";
import { useVodSearch } from "./useVodSearch";

export function useShowSearch(query: string): Async<SeriesDto[]> {
  return useVodSearch(query, backend.searchSeries);
}
