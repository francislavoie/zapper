// Opens a show's sheet from anywhere: the band's Details, while an episode of
// it plays under Live TV or Movies, goes to the Shows tab, selects the show
// (the sheet is about the selected one) and opens it there (openShowSheet).

import { openShowSheet } from "../actions";
import { useDispatch } from "../store";
import type { SeriesDto } from "../types";

export function useOpenShowSheet(show: SeriesDto): () => void {
  const dispatch = useDispatch();
  return () => openShowSheet(dispatch, show);
}
