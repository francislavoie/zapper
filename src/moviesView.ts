// Remembering an on-demand tab's List/Posters choice, the side-effecting half;
// the parsing is pure and lives in src/logic/movies.ts. Per computer, like the
// stacked layout, and for the same reasons. Each kind keeps its own choice
// under its own key.

import { MOVIES_VIEW_STORAGE_KEY, parseMoviesView, type MoviesView } from "./logic/movies";

function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function loadVodView(key: string): MoviesView {
  try {
    return parseMoviesView(storage()?.getItem(key) ?? null);
  } catch {
    return "list";
  }
}

export function saveVodView(key: string, v: MoviesView): void {
  try {
    storage()?.setItem(key, v);
  } catch {
    // Remembering failed; the view still changed.
  }
}

export function loadMoviesView(): MoviesView {
  return loadVodView(MOVIES_VIEW_STORAGE_KEY);
}

export function saveMoviesView(v: MoviesView): void {
  saveVodView(MOVIES_VIEW_STORAGE_KEY, v);
}
