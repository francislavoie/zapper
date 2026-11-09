// The Movies and Shows search's rule, for the mock catalogues: the real one is
// crates/zapper/src/catalog/title_search.rs, run inside the list's own query.
// A query matches a name when its text appears in the name at the start of a
// word (the start of the name, or after anything that is not a letter or a
// digit), case and accents aside, a run of spaces counting as one. `a|b` is
// either. So `EN -` finds "EN - A Movie" and "FR/EN - A Movie", not
// "Garden - A Movie". Pure; exercised by src/__checks__/titleMatch.test.mjs.
//
// Accents go by Unicode decomposition here; the Rust side also folds the few
// letters that have none (ø, ł…), which no mock title uses.

import { alternatives } from "./searchQuery.ts";

/** Lower case, without accents, each run of whitespace one space, trimmed. */
export function foldTitle(s: string): string {
  return s.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase().replace(/\s+/gu, " ").trim();
}

/** The alternatives a query asks for (searchQuery.ts), folded; none is no
 *  search. */
export function titleNeedles(query: string): string[] {
  return alternatives(query)
    .map(foldTitle)
    .filter((a) => a !== "");
}

/** Whether the text asks for anything: only pipes and spaces do not. */
export function isTitleSearch(text: string): boolean {
  return titleNeedles(text).length > 0;
}

const WORD = /[\p{L}\p{N}]/u;

/** Whether `name` matches any of `needles` (from titleNeedles); with none,
 *  everything does. */
export function titleMatches(needles: readonly string[], name: string): boolean {
  if (needles.length === 0) return true;
  const hay = foldTitle(name);
  return needles.some((needle) => {
    for (let i = hay.indexOf(needle); i >= 0; i = hay.indexOf(needle, i + 1)) {
      if (i === 0 || !WORD.test(hay[i - 1])) return true;
    }
    return false;
  });
}
