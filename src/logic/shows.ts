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
import { idle, loading, ready, type Async } from "./async.ts";
import {
  movieBadges,
  recallDetails,
  rememberDetails,
  runtime,
  vodQuery,
  type MovieRailKind,
  type PosterCard,
  type YearRange,
} from "./movies.ts";
import type { FixedRailItem } from "./railItems.ts";
import { OVERSCAN, computeWindow } from "./virtual.ts";

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

/** The list to ask for, as movieQuery: a search is every show whose name
 *  matches, in the chosen sort and years. */
export function showQuery(
  selection: string,
  sort: MovieSort,
  years: YearRange | null,
  search = "",
): SeriesQueryDto {
  return vodQuery(showRailKind(selection), selection, sort, years, search);
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
 *  first listed that is not the specials, else the specials (a show of
 *  nothing else). */
export function firstSeason(d: SeriesDetailsDto): number | null {
  const next = d.nextUp !== null ? d.episodes.find((x) => x.episodeId === d.nextUp) : undefined;
  return next?.season ?? (d.seasons.find((s) => s.number !== 0) ?? d.seasons[0])?.number ?? null;
}

/** How far into an episode it was left, 0–1, while it is in progress
 *  (1 %–92 %, as catalog/vod_progress.rs has it); `null` otherwise: not
 *  started, watched, or of no known length. */
export function episodeProgress(
  e: Pick<EpisodeDto, "watched" | "position" | "duration" | "durationSecs">,
): number | null {
  const length = e.duration ?? e.durationSecs;
  if (e.watched || e.position === null || !length || length <= 0) return null;
  const f = e.position / length;
  return f >= 0.01 && f < 0.92 ? f : null;
}

/** An episode row's height in the Show sheet, in pixels: a 63-pixel still
 *  and its padding. */
export const EPISODE_ROW_HEIGHT = 84;

/** Which of a season's episodes are drawn: those in sight and a few either
 *  side, so a season of hundreds stays a few dozen rows in the page. */
export function episodeWindow(
  scrollTop: number,
  height: number,
  count: number,
): { start: number; end: number; totalHeight: number } {
  const w = computeWindow(scrollTop, height, EPISODE_ROW_HEIGHT, count, OVERSCAN);
  return { start: w.start, end: w.end, totalHeight: w.totalHeight };
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

/** A show's details as kept, with the revision they were read at. */
export interface KeptDetails<T> {
  revision: number;
  details: T;
}

/** The shows' details, kept per show (the `max` most recent) with the
 *  revision each was read at, and a generation that `clear` moves on: an
 *  answer to a question asked before the clear (another account's shows)
 *  is not kept, nor one older than what is already kept. A question on its
 *  way is shared: the band and the sheet asking for the same show at once
 *  ask the backend once. */
export function showDetailsCache<T>(max: number) {
  const kept = new Map<number, KeptDetails<T>>();
  const asking = new Map<string, Promise<T>>();
  let generation = 0;
  return {
    /** Taken when a question is asked, handed back with its answer. */
    generation: () => generation,
    /** The question for `id` at `revision` in this generation: the one on
     *  its way if there is one, else `start`'s, until it settles. */
    join(id: number, revision: number, start: () => Promise<T>): Promise<T> {
      const key = `${id}:${revision}:${generation}`;
      const out = asking.get(key);
      if (out) return out;
      const p = start();
      const settled = () => {
        if (asking.get(key) === p) asking.delete(key);
      };
      p.then(settled, settled);
      asking.set(key, p);
      return p;
    },
    /** What is kept for `id`, without making it the newest. */
    peek: (id: number): KeptDetails<T> | undefined => kept.get(id),
    /** What is kept for `id`; asking makes it the newest. */
    recall: (id: number): KeptDetails<T> | undefined => recallDetails(kept, id),
    /** Keeps an answer read at `revision` to a question asked in `asked`;
     *  whether it was kept. */
    remember(id: number, revision: number, details: T, asked: number): boolean {
      if (asked !== generation) return false;
      const now = kept.get(id);
      if (now && now.revision > revision) return false;
      rememberDetails(kept, id, { revision, details }, max);
      return true;
    },
    clear(): void {
      kept.clear();
      asking.clear();
      generation++;
    },
    size: () => kept.size,
  };
}

/** One view's own answer about a show: which show, at which revision, in
 *  which generation it was asked. */
export interface DetailsAnswer<T> {
  id: number;
  revision: number;
  generation: number;
  value: Async<T>;
}

/** What a view shows of show `id`'s details at `revision`, in the cache's
 *  `generation`: the copy kept at this revision; else this view's own
 *  answer, once it has one; else, while a newer revision loads or when it
 *  failed, the older copy kept, so the sheet does not blank on every mark;
 *  else this view's loading or failure. */
export function detailsShown<T>(
  id: number | null,
  revision: number,
  generation: number,
  known: KeptDetails<T> | undefined,
  mine: DetailsAnswer<T> | null,
): Async<T> {
  if (id === null) return idle();
  if (known?.revision === revision) return ready(known.details);
  const own =
    mine?.id === id && mine.revision === revision && mine.generation === generation ? mine.value : null;
  if (own?.status === "ready") return own;
  if (known) return ready(known.details);
  return own ?? loading();
}
