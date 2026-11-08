// What the Movies tab says about a title and how it lays them out. The rail's
// own items share one selection string with the provider's categories, as on
// Live TV (see railItems.ts): the starred ids below cannot collide with a
// provider's id. Pure; exercised by src/__checks__/movies.test.mjs.

import type {
  MovieDetailsDto,
  MovieDto,
  MovieQueryDto,
  MovieSort,
  MoviesSummaryDto,
  PlayerStateDto,
} from "../types";
import type { Async } from "./async.ts";
import { runtime } from "./format.ts";
import { CONTINUE, type FixedRailItem } from "./railItems.ts";
import { progressOf } from "./resume.ts";
import { isTitleSearch } from "./titleMatch.ts";

/** Lengths are format.ts's; kept importable from here. */
export { runtime };

export const MOVIE_FAVOURITES = "*favourites*";
export const MOVIE_RECENT = "*added*";
export const MOVIE_WATCHED = "*watched*";

export type MovieRailKind = "favourites" | "recent" | "watched" | "category";

export function movieRailKind(id: string): MovieRailKind {
  switch (id) {
    case MOVIE_FAVOURITES:
      return "favourites";
    case MOVIE_RECENT:
      return "recent";
    case MOVIE_WATCHED:
      return "watched";
    default:
      return "category";
  }
}

/** Whether a summary says the movies are another account's than those shown
 *  so far (`shown`): a title with the same id is then another film. The
 *  first summary, or one that did not arrive, is no change. */
export function moviesOriginChanged(
  shown: Async<MoviesSummaryDto>,
  next: Async<MoviesSummaryDto>,
): boolean {
  return shown.status === "ready" && next.status === "ready" && shown.data.origin !== next.data.origin;
}

/** Whether the account has movies, which is what shows the Movies tab. Only
 *  an answer says so: until one arrives the tab is not there. */
export function moviesAvailable(summary: Async<MoviesSummaryDto>): boolean {
  return summary.status === "ready" && summary.data.available;
}

const countText = (n: number | null | undefined) => (n ? n.toLocaleString() : "");

/** The rail's own items, in order. A count is blank until there is something
 *  to count. Continue watching comes first when `continueCount` is given
 *  (blank while it is 0). */
export function fixedMovieItems(
  s: MoviesSummaryDto | null,
  continueCount?: number | null,
): FixedRailItem[] {
  const first: FixedRailItem[] =
    continueCount === undefined || continueCount === null
      ? []
      : [{ id: CONTINUE, name: "Continue watching", count: countText(continueCount) }];
  return [
    ...first,
    { id: MOVIE_FAVOURITES, name: "Favourites", count: countText(s?.favourites) },
    { id: MOVIE_RECENT, name: "Recently added", count: countText(s?.recent) },
    { id: MOVIE_WATCHED, name: "Watched", count: countText(s?.watched) },
  ];
}

/** The search box's placeholder: what it searches, the syntax in
 *  brief (the key that gets there is the box's key hint). */
export const MOVIE_SEARCH_PLACEHOLDER = "Search movies  ·  a|b for either";

export const MOVIE_SORTS: readonly MovieSort[] = ["added", "az", "year", "rating"];

export const MOVIE_SORT_LABEL: Record<MovieSort, string> = {
  added: "Recently added",
  az: "A–Z",
  year: "Year",
  rating: "Rating",
};

/** Inclusive; a null end is open. */
export interface YearRange {
  from: number | null;
  to: number | null;
}

/** The year filter's choices: everything, the last two years, each decade
 *  down to the 1980s, and what came before. */
export function yearRanges(nowYear: number): { label: string; range: YearRange | null }[] {
  const out: { label: string; range: YearRange | null }[] = [
    { label: "All years", range: null },
    { label: `${nowYear - 1}–${nowYear}`, range: { from: nowYear - 1, to: nowYear } },
  ];
  for (let d = Math.floor(nowYear / 10) * 10; d >= 1980; d -= 10) {
    out.push({ label: `${d}s`, range: { from: d, to: d + 9 } });
  }
  out.push({ label: "Before 1980", range: { from: null, to: 1979 } });
  return out;
}

/** The list to ask for. `search` is the text in the search box once the
 *  typing has paused: while there is any, the list is every movie whose
 *  name matches, in the chosen sort and years, whatever is chosen in the
 *  rail. The search is part of the query, so the query is also what tells
 *  one list from another (a new search starts at the top, as a new category
 *  does). */
export function movieQuery(
  selection: string,
  sort: MovieSort,
  years: YearRange | null,
  search = "",
): MovieQueryDto {
  return vodQuery(movieRailKind(selection), selection, sort, years, search);
}

/** movieQuery, for either tab: the rail's kind of list and its id. */
export function vodQuery(
  kind: MovieRailKind,
  selection: string,
  sort: MovieSort,
  years: YearRange | null,
  search: string,
): MovieQueryDto {
  const yearFrom = years?.from ?? null;
  const yearTo = years?.to ?? null;
  if (isTitleSearch(search)) {
    return { list: "all", categoryId: null, sort, yearFrom, yearTo, search: search.trim() };
  }
  return {
    list: kind,
    categoryId: kind === "category" ? selection : null,
    sort,
    yearFrom,
    yearTo,
    search: null,
  };
}

export function movieTitle(
  selection: string,
  categories: readonly { id: string; name: string }[],
): string {
  switch (movieRailKind(selection)) {
    case "favourites":
      return "Favourites";
    case "recent":
      return "Recently added";
    case "watched":
      return "Watched";
    default:
      return categories.find((c) => c.id === selection)?.name ?? "Movies";
  }
}

export function movieCount(n: number): string {
  return `${n.toLocaleString()} ${n === 1 ? "movie" : "movies"}`;
}

/** "2025 · 1h 49m · ★ 7.6", with whatever is unknown left out. */
export function movieFacts(m: Pick<MovieDto, "year" | "durationSecs" | "rating">): string {
  const parts: string[] = [];
  if (m.year) parts.push(String(m.year));
  const r = runtime(m.durationSecs);
  if (r) parts.push(r);
  if (m.rating !== null) parts.push(`★ ${m.rating.toFixed(1)}`);
  return parts.join(" · ");
}

/** How long a title counts as new. */
export const NEW_DAYS = 7;

const UHD = /\b(4K|UHD|2160p)\b/i;

/** Small marks beside a title: added within the week, and 4K when the
 *  provider's name says so (there is no resolution in the list data). */
export function movieBadges(
  m: Pick<MovieDto, "name" | "added">,
  now: number,
): ("NEW" | "4K")[] {
  const out: ("NEW" | "4K")[] = [];
  if (m.added !== null && now - m.added < NEW_DAYS * 86_400) out.push("NEW");
  if (UHD.test(m.name)) out.push("4K");
  return out;
}

/** What a poster card draws, whatever kind of title it is: the clean name
 *  on the art, the full one for its tip, and one line of facts under it. */
export interface PosterCard {
  /** A title's id; a Continue watching entry's key. */
  id: number | string;
  /** The clean name, drawn. */
  title: string;
  /** The full name, for data-tip-overflow. */
  name: string;
  posterUrl: string | null;
  badges: string[];
  meta: string;
  watched: boolean;
  favourite: boolean;
  /** How far along, 0-1, while in progress. */
  progress: number | null;
}

/** A movie as a poster card: "2025 · 1h 49m · ★ 7.6" under it. */
export function movieCard(m: MovieDto, now: number): PosterCard {
  return {
    id: m.movieId,
    title: m.cleanName,
    name: m.name,
    posterUrl: m.posterUrl,
    badges: movieBadges(m, now),
    meta: movieFacts(m),
    watched: m.watched,
    favourite: m.favourite,
    progress: progressOf({
      watched: m.watched,
      position: m.position ?? null,
      duration: m.duration ?? null,
      durationSecs: m.durationSecs,
    }),
  };
}

/** "Director X · With A, B, C, D": the first four of the cast. */
export function peopleLine(d: Pick<MovieDetailsDto, "director" | "cast">): string {
  const parts: string[] = [];
  if (d.director) parts.push(`Director ${d.director}`);
  if (d.cast.length > 0) parts.push(`With ${d.cast.slice(0, 4).join(", ")}`);
  return parts.join(" · ");
}

/** "EN · FR audio · subtitles EN, FR", with empty parts left out. */
export function languagesLine(
  d: Pick<MovieDetailsDto, "audioLanguages" | "subtitleLanguages">,
): string {
  const parts: string[] = [];
  if (d.audioLanguages.length > 0) {
    parts.push(`${d.audioLanguages.map((l) => l.toUpperCase()).join(" · ")} audio`);
  }
  if (d.subtitleLanguages.length > 0) {
    parts.push(`subtitles ${d.subtitleLanguages.map((l) => l.toUpperCase()).join(", ")}`);
  }
  return parts.join(" · ");
}

export const MOVIES_VIEW_STORAGE_KEY = "zapper.moviesView";

export type MoviesView = "list" | "posters";

/** A list unless posters were chosen on this computer. */
export function parseMoviesView(raw: string | null): MoviesView {
  return raw === "posters" ? "posters" : "list";
}

export const POSTER_MIN_WIDTH = 150;
export const POSTER_GAP = 14;
/** Room under a poster for its title and facts. */
export const POSTER_CAPTION = 44;

/** Whole columns of posters across `width`, each stretched to share the
 *  leftover. A poster is two thirds as wide as it is tall. */
export function posterGrid(
  width: number,
  total: number,
): { columns: number; cardWidth: number; rowHeight: number; rows: number } {
  const columns = Math.max(1, Math.floor((width + POSTER_GAP) / (POSTER_MIN_WIDTH + POSTER_GAP)));
  const cardWidth = Math.floor((width - POSTER_GAP * (columns - 1)) / columns);
  return {
    columns,
    cardWidth,
    rowHeight: Math.round(cardWidth * 1.5) + POSTER_CAPTION + POSTER_GAP,
    rows: Math.ceil(total / columns),
  };
}

/** What the player has open. A title has no stream id, so `onDemand` decides
 *  before the stream id does. */
export function playingKind(
  p: Pick<PlayerStateDto, "state" | "streamId" | "onDemand">,
): "channel" | "movie" | "episode" | null {
  if (p.state === "stopped") return null;
  if (p.onDemand) return p.onDemand.kind;
  return p.streamId !== null ? "channel" : null;
}

/** How long a title stays selected before its details are fetched: a
 *  held arrow key crosses rows without asking the provider about each. */
export const DETAILS_DELAY_MS = 400;

/** Remembers `d` as the answer for `id`, forgetting the oldest answers past
 *  `max`. A Map keeps its keys in the order they were set, so the first key
 *  is the oldest. Changes and returns `cache`. A movie's details or a show's
 *  alike. */
export function rememberDetails<T>(
  cache: Map<number, T>,
  id: number,
  d: T,
  max: number,
): Map<number, T> {
  cache.delete(id);
  cache.set(id, d);
  while (cache.size > max) {
    const oldest = cache.keys().next();
    if (oldest.done) break;
    cache.delete(oldest.value);
  }
  return cache;
}

/** What was remembered for `id`; asking makes it the newest. */
export function recallDetails<T>(cache: Map<number, T>, id: number): T | undefined {
  const d = cache.get(id);
  if (d !== undefined) {
    cache.delete(id);
    cache.set(id, d);
  }
  return d;
}
