// Where a search result comes from.
//
// A channel name says little about which category it was found in, and big
// catalogues carry the same name in many — "NORTHWIND NEWS" in a country's
// news, its sports and its 4K section. So channel results are listed under
// their category, and programme results name it on every row.
//
// Checked in src/__checks__/searchGroups.test.mjs.

import type { CategoryDto, ChannelDto } from "../types";

/** What a channel in no category is filed under. */
export const NO_CATEGORY = "No category";

/** Category names by id, for labelling results. */
export function categoryNames(categories: readonly CategoryDto[]): Map<string, string> {
  return new Map(categories.map((c) => [c.id, c.name]));
}

/** A channel's category, by name. An id the rail does not know yet (the
 *  categories still loading) reads as no category rather than as an id. */
export function categoryOf(names: ReadonlyMap<string, string>, id: string | null): string {
  return (id !== null ? names.get(id) : undefined) ?? NO_CATEGORY;
}

/** Ranked channel results, regrouped under a heading row per category.
 *
 *  The ranking still decides the order: a category comes where its best hit
 *  ranked, and its channels follow in their own ranked order. So the best
 *  match is still the first channel in the list, and a weak hit in a
 *  category that holds a strong one is pulled up beside it.
 *
 *  The headings are separator rows — what the list already draws, pins at
 *  the top while scrolling, and steps over when zapping — with negative ids,
 *  which no channel has, so they key apart from the channels. */
export function groupByCategory(
  ranked: readonly ChannelDto[],
  names: ReadonlyMap<string, string>,
): ChannelDto[] {
  // By name rather than id: two headings saying the same thing would read
  // as a mistake, whether they are two unknown ids or two categories a
  // provider happened to name alike.
  const groups = new Map<string, { id: string | null; channels: ChannelDto[] }>();
  for (const c of ranked) {
    if (c.isSeparator) continue;
    const name = categoryOf(names, c.categoryId);
    const g = groups.get(name);
    if (g) g.channels.push(c);
    else groups.set(name, { id: c.categoryId, channels: [c] });
  }
  const out: ChannelDto[] = [];
  let heading = 0;
  for (const [name, { id, channels }] of groups) {
    heading += 1;
    out.push({
      streamId: -heading,
      name,
      cleanName: name,
      num: null,
      logoUrl: null,
      categoryId: id,
      hasArchive: false,
      archiveDays: null,
      isSeparator: true,
      quality: null,
      nowPlaying: null,
    });
    out.push(...channels);
  }
  return out;
}
