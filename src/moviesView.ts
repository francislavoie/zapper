// Remembering the Movies tab's List/Posters choice, the side-effecting half;
// the parsing is pure and lives in src/logic/movies.ts. Per computer, like the
// stacked layout, and for the same reasons.

import { MOVIES_VIEW_STORAGE_KEY, parseMoviesView, type MoviesView } from "./logic/movies";

function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function loadMoviesView(): MoviesView {
  try {
    return parseMoviesView(storage()?.getItem(MOVIES_VIEW_STORAGE_KEY) ?? null);
  } catch {
    return "list";
  }
}

export function saveMoviesView(v: MoviesView): void {
  try {
    storage()?.setItem(MOVIES_VIEW_STORAGE_KEY, v);
  } catch {
    // Remembering failed; the view still changed.
  }
}
