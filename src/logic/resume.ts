// Where a movie or episode was left, and what is said of it. A title is in
// progress from 1 % to 92 % of its length: the catalogue's own line
// (`STARTED_FRACTION` and `WATCHED_FRACTION`, catalog/vod_progress.rs),
// written once here for the interface. Pure; exercised by
// src/__checks__/resume.test.mjs.

import { calendarDate, clock, hhmm, runtime } from "./format.ts";
import type { MarksDto } from "../types.ts";

export const STARTED_FRACTION = 0.01;
export const WATCHED_FRACTION = 0.92;

/** What a movie or an episode says about where it was left. */
export interface Place {
  watched: boolean;
  /** Seconds from the start. */
  position: number | null;
  /** The length the player measured. */
  duration: number | null;
  /** The provider's length. */
  durationSecs: number | null;
}

/** How far along, 0–1, while in progress; `null` when not started, watched,
 *  or of no known length. The player's length wins over the provider's. */
export function progressOf(t: Place): number | null {
  const length = t.duration ?? t.durationSecs;
  if (t.watched || t.position == null || !length || length <= 0) return null;
  const f = t.position / length;
  return f >= STARTED_FRACTION && f < WATCHED_FRACTION ? f : null;
}

/** Where Resume starts, while the title is in progress. */
export function resumePoint(t: Place): number | null {
  return progressOf(t) === null ? null : t.position;
}

/** Where Up next starts the next episode: where it was left while in
 *  progress (`listedSecs`, the provider's length, when the player measured
 *  none), else its beginning. */
export function nextStart(marks: MarksDto | null, listedSecs: number | null): number {
  if (!marks) return 0;
  return resumePoint({ ...marks, durationSecs: listedSecs }) ?? 0;
}

/** Whether a film read again (after a break, a seek, another sound, its
 *  viewer let go) plays once it is ready. Started, as the picture was:
 *  playing, or run out of what it had (`ended`, which the element also calls
 *  paused); with no element (`paused: null`), playing. Not started yet, as it
 *  was asked to (`wantPlay`): a title held for the Resume question stays
 *  paused behind it. */
export function playsOnReopen(p: {
  started: boolean;
  paused: boolean | null;
  ended: boolean;
  wantPlay: boolean;
}): boolean {
  if (!p.started) return p.wantPlay;
  return p.paused === null || !p.paused || p.ended;
}

/** "14m left", at least "1m left"; nothing when not in progress. */
export function timeLeft(t: Place): string {
  if (progressOf(t) === null) return "";
  const length = (t.duration ?? t.durationSecs) as number;
  return `${runtime(Math.max(60, length - (t.position as number)))} left`;
}

const dayOf = (unixSeconds: number): number => {
  const d = new Date(unixSeconds * 1000);
  return Math.round(new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime() / 86_400_000);
};

/** When, in a few words: "today", "yesterday", the short weekday within
 *  six days, "28 Oct" in the same year, else the full date. A moment ahead of
 *  `now` (a clock that is behind) is today. */
export function watchedWhen(at: number, now: number): string {
  const days = Math.max(0, dayOf(now) - dayOf(at));
  if (days === 0) return "today";
  if (days === 1) return "yesterday";
  const d = new Date(at * 1000);
  if (days <= 6) return d.toLocaleDateString(undefined, { weekday: "short" });
  if (d.getFullYear() === new Date(now * 1000).getFullYear()) {
    return d.toLocaleDateString(undefined, { day: "numeric", month: "short" });
  }
  return calendarDate(at);
}

/** "46m watched · 1h 15m left · yesterday 22:40"; the when is left out with
 *  no `playedAt`. */
export function watchedLine(t: Place, playedAt: number | null, now: number): string {
  const parts: string[] = [];
  if (progressOf(t) !== null) {
    parts.push(`${runtime(Math.max(60, t.position as number))} watched`, timeLeft(t));
  }
  if (playedAt !== null) parts.push(`${watchedWhen(playedAt, now)} ${hhmm(playedAt)}`);
  return parts.join(" · ");
}

/** "Resume from 46:10". */
export function resumeLabel(position: number): string {
  return `Resume from ${clock(position)}`;
}

/** How an open starts: a number is there, `null` the beginning, "resume" the
 *  place left (the beginning without one), "ask" the place left held paused
 *  for the question (the beginning, and no question, without one). */
export type StartAt = number | null | "ask" | "resume";

export function startPlan(
  choice: StartAt,
  place: Place | null,
): { start: number | null; held: boolean; ask: boolean } {
  if (choice === null || typeof choice === "number") return { start: choice, held: false, ask: false };
  const point = place ? resumePoint(place) : null;
  if (choice === "resume" || point === null) return { start: point, held: false, ask: false };
  return { start: point, held: true, ask: true };
}

/** The question a held open asks. */
export interface ResumePrompt {
  item: { kind: "movie" | "episode"; id: number };
  title: string;
  position: number;
  playedAt: number | null;
}

/** Where the question stands for the player's state: shown while the prompt's
 *  item is open and opening or paused; `seen` once it has been; `clear` when
 *  its item plays, fails or stops, or when it was seen and the player has
 *  anything else. Before the player first reports the item nothing is
 *  cleared: the open is on its way. */
export function promptStep(
  prompt: ResumePrompt | null,
  seen: boolean,
  p: { state: string; onDemand: { kind: string; id: number } | null },
): { shown: boolean; seen: boolean; clear: boolean } {
  if (prompt === null) return { shown: false, seen: false, clear: false };
  const open = p.onDemand?.kind === prompt.item.kind && p.onDemand.id === prompt.item.id;
  if (open && (p.state === "opening" || p.state === "paused")) return { shown: true, seen: true, clear: false };
  // Until the title has been reported opening or paused, whatever the player
  // says is from before the open: nothing clears it.
  return { shown: false, seen, clear: seen };
}

/** Whether the question is seen, for the prompt now asked: a new prompt (or
 *  another item) starts unseen, whatever the last one was. */
export function seenFor(last: ResumePrompt | null, seen: boolean, now: ResumePrompt | null): boolean {
  return last !== null && last === now ? seen : false;
}

/** "You stopped at 46:10, yesterday at 22:40." */
export function promptLine(p: ResumePrompt, now: number): string {
  const at = clock(p.position);
  return p.playedAt === null
    ? `You stopped at ${at}.`
    : `You stopped at ${at}, ${watchedWhen(p.playedAt, now)} at ${hhmm(p.playedAt)}.`;
}
