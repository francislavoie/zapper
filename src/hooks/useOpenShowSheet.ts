// Opens a show's sheet from anywhere: the band's Details, while an episode of
// it plays under Live TV or Movies, goes to the Shows tab, selects the show
// (the sheet is about the selected one) and opens it there.

import { useApp, useDispatch } from "../store";
import type { SeriesDto } from "../types";

export function useOpenShowSheet(show: SeriesDto): () => void {
  const { section, shows } = useApp();
  const dispatch = useDispatch();
  return () => {
    if (section !== "shows") dispatch({ type: "section", section: "shows" });
    if (shows.selected?.seriesId !== show.seriesId) dispatch({ type: "shows/selectShow", show });
    dispatch({ type: "shows/sheet", id: show.seriesId });
  };
}
