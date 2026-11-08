// What Continue watching says about an entry: a movie in progress, an
// episode in progress, or the next episode of a show being followed. One
// entry per movie and per show. Pure; exercised by
// src/__checks__/continueWatching.test.mjs.

import type { ContinueItemDto, ContinueKind } from "../types";
import { runtime, type PosterCard } from "./movies.ts";
import { progressOf, timeLeft, watchedWhen, type Place } from "./resume.ts";
import { episodeCode, episodeTitle } from "./shows.ts";

/** The rail's own item, the same id on both tabs: starred, so never a
 *  provider category's. */
export const CONTINUE = "*continue*";

/** The most the catalogue lists. */
export const CONTINUE_MAX = 100;

export const CONTINUE_EMPTY =
  "Nothing to continue yet. A movie or episode you stop partway, and the next episode of a show you're watching, appear here.";

/** How long the page is hidden before coming back to it asks again. */
export const FRESH_AFTER_HIDDEN_MS = 30_000;

/** Whether coming back after being hidden since `hiddenAt` should read the
 *  list again. */
export function freshOnReturn(hiddenAt: number | null, now: number): boolean {
  return hiddenAt !== null && now - hiddenAt > FRESH_AFTER_HIDDEN_MS;
}

/** Continue watching is the first selection when it has entries and nothing
 *  has been chosen yet (still on the page's own first one, `recent`). */
export function firstSelection(
  chosen: boolean,
  selection: string,
  recent: string,
  items: number,
): string | null {
  return !chosen && selection === recent && items > 0 ? CONTINUE : null;
}

/** "movie:7", "show:12": a show is one entry whichever episode it is at. */
export function continueKey(i: ContinueItemDto): string {
  return i.kind === "movie" ? `movie:${i.movie.movieId}` : `show:${i.series.seriesId}`;
}

/** What a removal names. */
export function continueTarget(i: ContinueItemDto): { kind: ContinueKind; id: number } {
  return i.kind === "movie"
    ? { kind: "movie", id: i.movie.movieId }
    : { kind: "series", id: i.series.seriesId };
}

export function continuePlace(i: ContinueItemDto): Place {
  const t = i.kind === "movie" ? i.movie : i.episode;
  return {
    watched: t.watched,
    position: t.position ?? null,
    duration: t.duration ?? null,
    durationSecs: t.durationSecs ?? null,
  };
}

/** The clean name of the movie or the show. */
export function continueTitle(i: ContinueItemDto): string {
  return i.kind === "movie" ? i.movie.cleanName : i.series.cleanName;
}

/** "", "S2 E4", "S1 E3 · next". */
export function continueCode(i: ContinueItemDto): string {
  if (i.kind === "movie") return "";
  const code = episodeCode(i.episode.season, i.episode.episode, i.episode.episodeEnd);
  return i.upNext ? `${code} · next` : code;
}

const FROM_THE_BEGINNING = "Starts from the beginning";

/** What is left to watch, or that it starts over. */
function leftOf(i: ContinueItemDto): string {
  const left = timeLeft(continuePlace(i));
  return left !== "" ? left : i.kind === "episode" && i.upNext ? FROM_THE_BEGINNING : "";
}

/** An entry as a poster card: the show's poster for an episode, and
 *  "S2 E4 · 14m left" under it. */
export function continueCard(i: ContinueItemDto, _now: number): PosterCard {
  const t = i.kind === "movie" ? i.movie : i.series;
  const code = i.kind === "episode" ? episodeCode(i.episode.season, i.episode.episode, i.episode.episodeEnd) : "";
  return {
    id: continueKey(i),
    title: continueTitle(i),
    name: t.name,
    posterUrl: t.posterUrl,
    badges: [],
    meta: [code, leftOf(i)].filter((p) => p !== "").join(" · "),
    watched: t.watched,
    favourite: t.favourite,
    progress: progressOf(continuePlace(i)),
  };
}

/** What a strip card draws. `art` is the episode's still, else the poster.
 *  An episode's `title` is its own (a "Episode 4" when it has none), its
 *  `line` the show and the code; a movie's `line` is its year and length. */
export interface StripCard {
  key: string;
  art: string | null;
  title: string;
  line: string;
  left: string;
  when: string;
  progress: number | null;
}

export function stripCard(i: ContinueItemDto, now: number): StripCard {
  const common = {
    key: continueKey(i),
    left: leftOf(i),
    when: watchedWhen(i.lastWatched, now),
    progress: progressOf(continuePlace(i)),
  };
  if (i.kind === "movie") {
    const facts = [i.movie.year ? String(i.movie.year) : "", runtime(i.movie.durationSecs)];
    return {
      ...common,
      art: i.movie.posterUrl,
      title: i.movie.cleanName,
      line: facts.filter((p) => p !== "").join(" · "),
    };
  }
  return {
    ...common,
    art: i.episode.stillUrl ?? i.series.posterUrl,
    title: episodeTitle(i.episode),
    line: `${i.series.cleanName} · ${episodeCode(i.episode.season, i.episode.episode, i.episode.episodeEnd)}`,
  };
}

/** "1 title", "4 titles". */
export function continueCount(n: number): string {
  return `${n.toLocaleString()} ${n === 1 ? "title" : "titles"}`;
}
