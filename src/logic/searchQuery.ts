// What a search box accepts beyond plain words.
//
// `|` offers alternatives: `news|weather` finds either. It binds loosest, so
// each alternative is a whole query and its words must all match —
// `news|weather uk` is "news", or "weather" and "uk" together. The backend
// reads the same syntax for channels (crates/zapper-core/src/catalog/search.rs) and
// programmes (guide_search.rs); this is the frontend's half, for the
// palette's action filter and the mock, and the words the help shows.
//
// Checked in src/__checks__/searchQuery.test.mjs.

/** The alternatives in a query: split on `|`, trimmed, empty ones dropped.
 *  Empty when there is nothing to look for, pipes or not. */
export function alternatives(query: string): string[] {
  return query
    .split("|")
    .map((a) => a.trim())
    .filter((a) => a !== "");
}

/** Whether any alternative of `query` satisfies `matches`. An empty query
 *  matches everything, as an empty search box always has. */
export function anyAlternative(query: string, matches: (alternative: string) => boolean): boolean {
  const alts = alternatives(query);
  return alts.length === 0 || alts.some(matches);
}

/** The syntax, as the help lists it: an example and what it finds. */
export const SEARCH_SYNTAX: readonly { example: string; finds: string }[] = [
  { example: "harbour news", finds: "Both words, in any order" },
  { example: "news|weather", finds: "Either one" },
  { example: "news|weather uk", finds: "“news”, or “weather” with “uk”" },
];

/** The list's search box placeholder: what it searches, the syntax in
 *  brief. The key that gets there is the box's key hint. */
export const SEARCH_PLACEHOLDER = "Search channels  ·  a|b for either";
