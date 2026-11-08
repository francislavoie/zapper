// What the Shows tab says about a show and its episodes. The rail, lists and
// sorts are the movies' (see movies.ts), with a show's own facts: its years
// and seasons rather than a running time, and an episode's code. Pure;
// exercised by src/__checks__/shows.test.mjs.

import type {
  EpisodeDto,
  MovieSort,
  OnDemandDto,
  PlayerStateDto,
  SeasonDto,
  SeriesDetailsDto,
  SeriesDto,
  SeriesQueryDto,
  ShowsSummaryDto,
} from "../types";
import type { Async } from "./async.ts";
import { movieBadges, runtime, type MovieRailKind, type PosterCard, type YearRange } from "./movies.ts";
import type { FixedRailItem } from "./railItems.ts";

export const SHOW_FAVOURITES = "*favourites*";
export const SHOW_RECENT = "*added*";
export const SHOW_WATCHED = "*watched*";

export function showRailKind(id: string): MovieRailKind {
  switch (id) {
    case SHOW_FAVOURITES:
      return "favourites";
    case SHOW_RECENT:
      return "recent";
    case SHOW_WATCHED:
      return "watched";
    default:
      return "category";
  }
}

/** Whether the account has shows, which is what shows the Shows tab. Only an
 *  answer says so: until one arrives the tab is not there. */
export function showsAvailable(summary: Async<ShowsSummaryDto>): boolean {
  return summary.status === "ready" && summary.data.available;
}

/** Whether a summary says the shows are another account's than those shown
 *  so far: a show with the same id is then another show. The first summary,
 *  or one that did not arrive, is no change. */
export function showsOriginChanged(
  shown: Async<ShowsSummaryDto>,
  next: Async<ShowsSummaryDto>,
): boolean {
  return shown.status === "ready" && next.status === "ready" && shown.data.origin !== next.data.origin;
}

const countText = (n: number | null | undefined) => (n ? n.toLocaleString() : "");

/** The rail's own items, in order. A count is blank until there is something
 *  to count. */
export function fixedShowItems(s: ShowsSummaryDto | null): FixedRailItem[] {
  return [
    { id: SHOW_FAVOURITES, name: "Favourites", count: countText(s?.favourites) },
    { id: SHOW_RECENT, name: "Recently added", count: countText(s?.recent) },
    { id: SHOW_WATCHED, name: "Watched", count: countText(s?.watched) },
  ];
}

/** The search box's placeholder (the key that gets there is the box's key
 *  hint). */
export const SHOW_SEARCH_PLACEHOLDER = "Search shows  ·  a|b for either";

export const SHOWS_VIEW_STORAGE_KEY = "zapper.showsView";

export function showQuery(
  selection: string,
  sort: MovieSort,
  years: YearRange | null,
): SeriesQueryDto {
  const kind = showRailKind(selection);
  return {
    list: kind,
    categoryId: kind === "category" ? selection : null,
    sort,
    yearFrom: years?.from ?? null,
    yearTo: years?.to ?? null,
  };
}

export function showTitle(
  selection: string,
  categories: readonly { id: string; name: string }[],
): string {
  switch (showRailKind(selection)) {
    case "favourites":
      return "Favourites";
    case "recent":
      return "Recently added";
    case "watched":
      return "Watched";
    default:
      return categories.find((c) => c.id === selection)?.name ?? "Shows";
  }
}

export function showCount(n: number): string {
  return `${n.toLocaleString()} ${n === 1 ? "show" : "shows"}`;
}

/** "1 season", "3 seasons"; nothing while the count is not known. */
export function seasonsText(n: number | null): string {
  if (!n) return "";
  return `${n} ${n === 1 ? "season" : "seasons"}`;
}

/** "2021", "2021–2024"; nothing without a first year. */
export function yearsText(first: number | null, last: number | null): string {
  if (first === null) return "";
  return last !== null && last !== first ? `${first}–${last}` : String(first);
}

/** "2021–2024 · 3 seasons · ★ 8.1", with whatever is unknown left out. The
 *  years are the episodes' once the details are here, else the show's own. */
export function showFacts(
  s: Pick<SeriesDto, "year" | "seasonCount" | "rating">,
  d?: Pick<SeriesDetailsDto, "firstYear" | "lastYear">,
): string {
  const parts: string[] = [];
  const years = yearsText(d?.firstYear ?? s.year, d?.lastYear ?? null);
  if (years) parts.push(years);
  const seasons = seasonsText(s.seasonCount);
  if (seasons) parts.push(seasons);
  if (s.rating !== null) parts.push(`★ ${s.rating.toFixed(1)}`);
  return parts.join(" · ");
}

/** A show as a poster card: "2021 · 3 seasons · ★ 8.1" under it. */
export function showCard(s: SeriesDto, now: number): PosterCard {
  return {
    id: s.seriesId,
    title: s.cleanName,
    name: s.name,
    posterUrl: s.posterUrl,
    badges: movieBadges(s, now),
    meta: showFacts(s),
    watched: s.watched,
    favourite: s.favourite,
  };
}

/** "S2 E4", a multi-episode entry "S1 E1–E2"; a special "Special 3". */
export function episodeCode(season: number, episode: number, end: number | null = null): string {
  const last = end !== null && end > episode ? end : null;
  if (season === 0) return last === null ? `Special ${episode}` : `Special ${episode}–${last}`;
  return last === null ? `S${season} E${episode}` : `S${season} E${episode}–E${last}`;
}

/** "Season 2"; season 0 is the specials. */
export function seasonLabel(n: number): string {
  return n === 0 ? "Specials" : `Season ${n}`;
}

/** The episode's own title, else "Episode 4". */
export function episodeTitle(e: Pick<EpisodeDto, "episode" | "title">): string {
  const t = e.title?.trim();
  return t ? t : `Episode ${e.episode}`;
}

/** "S2 E4 · Night Shift · 52m", with an unknown title or length left out. */
export function episodeLine(
  e: Pick<EpisodeDto, "season" | "episode" | "title" | "durationSecs"> &
    Partial<Pick<EpisodeDto, "episodeEnd" | "duration">>,
): string {
  const parts = [episodeCode(e.season, e.episode, e.episodeEnd ?? null)];
  const title = e.title?.trim();
  if (title) parts.push(title);
  const length = runtime(e.durationSecs ?? e.duration ?? null);
  if (length) parts.push(length);
  return parts.join(" · ");
}

/** "8 episodes · 3 watched"; "1 episode" while none is watched. */
export function seasonMeta(s: SeasonDto): string {
  const count = `${s.episodeCount} ${s.episodeCount === 1 ? "episode" : "episodes"}`;
  return s.watchedCount > 0 ? `${count} · ${s.watchedCount} watched` : count;
}

/** A show's main action: which episode it plays, and what it says. */
export type ShowAction = { label: string; episodeId: number } | null;

/** Plays the next-up episode; with none (all watched), the first one in
 *  order, the specials after the seasons; nothing without episodes. */
export function showAction(d: SeriesDetailsDto): ShowAction {
  const e =
    (d.nextUp !== null ? d.episodes.find((x) => x.episodeId === d.nextUp) : undefined) ??
    d.episodes.find((x) => x.season !== 0) ??
    d.episodes[0];
  if (!e) return null;
  return { label: `Play ${episodeCode(e.season, e.episode, e.episodeEnd)}`, episodeId: e.episodeId };
}

/** The season a show's episode list opens on: the next-up's, else the
 *  first listed. */
export function firstSeason(d: SeriesDetailsDto): number | null {
  const next = d.nextUp !== null ? d.episodes.find((x) => x.episodeId === d.nextUp) : undefined;
  return next?.season ?? d.seasons[0]?.number ?? null;
}

/** The episode the player holds: the one it has open, else the one that has
 *  just played to its end (`finished`, which only a finished end sets). The
 *  playing episode the store keeps lasts as long as this names it. */
export function heldEpisode(
  p: Pick<PlayerStateDto, "onDemand"> & { finished?: OnDemandDto | null },
): number | null {
  if (p.onDemand) return p.onDemand.kind === "episode" ? p.onDemand.id : null;
  return p.finished?.kind === "episode" ? p.finished.id : null;
}
