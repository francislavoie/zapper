// Up next: near an episode's end, a card counts down into the next one, and
// plays it when it gets to nothing. Which episode is next, and where the
// count stands, are worked out here from the player's state alone: the count
// is the playback position's, not a clock's, so pausing holds it, a seek
// back before the card hides it, and a seek into the last seconds makes it
// due at once. Pure; exercised by src/__checks__/upNext.test.mjs.

import type { EpisodeDto, PlayerStateDto } from "../types";

/** How long before an episode's end the card shows. */
export const UP_NEXT_LEAD_SECS = 20;
/** How long it counts down for, so the next one starts this long before
 *  the end: normally before the file runs out. */
export const UP_NEXT_COUNTDOWN_SECS = 10;

/** The episode after `currentId`: the next in the season, past the whole
 *  of a multi-episode entry, else the first of the next season; `null`
 *  after the last, for a special, and for an episode not listed. Specials
 *  are never next. Watched or not: someone watching again wants the next
 *  one, not a jump. */
export function nextEpisode(episodes: readonly EpisodeDto[], currentId: number): EpisodeDto | null {
  const current = episodes.find((e) => e.episodeId === currentId);
  if (!current || current.season === 0) return null;
  const last = Math.max(current.episode, current.episodeEnd ?? current.episode);
  let best: EpisodeDto | null = null;
  for (const e of episodes) {
    if (e.season === 0) continue;
    const after = e.season > current.season || (e.season === current.season && e.episode > last);
    if (!after) continue;
    if (best === null || e.season < best.season || (e.season === best.season && e.episode < best.episode)) {
      best = e;
    }
  }
  return best;
}

export type UpNext =
  | { kind: "none" }
  | { kind: "counting"; next: EpisodeDto; secondsLeft: number }
  | { kind: "due"; next: EpisodeDto };

const NONE: UpNext = { kind: "none" };

/** Where Up next stands for the episode the store has as playing
 *  (`current`), with `next` after it. The card shows from `duration −
 *  UP_NEXT_LEAD_SECS` and counts `UP_NEXT_COUNTDOWN_SECS` down to due; an
 *  episode that played to its end (`finished`) is due at once, whatever its
 *  length. Nothing for a movie, a special, no next, a length not known
 *  while it plays, a Cancel for this episode, a sleep timer that has ended
 *  or is fading playback out (`sleeping`), another title in the player, or
 *  any state but playing, paused, or stopped at a finished end: a drop, a
 *  Stop and an open never move on. */
export function upNextState(
  p: Pick<PlayerStateDto, "state" | "onDemand" | "finished">,
  current: EpisodeDto | null,
  next: EpisodeDto | null,
  cancelledFor: number | null,
  sleeping: boolean,
): UpNext {
  if (current === null || next === null || sleeping) return NONE;
  if (current.season === 0 || next.episodeId === current.episodeId) return NONE;
  if (cancelledFor === current.episodeId) return NONE;
  if (p.state === "stopped") {
    const f = p.finished;
    return f?.kind === "episode" && f.id === current.episodeId ? { kind: "due", next } : NONE;
  }
  if (p.state !== "playing" && p.state !== "paused") return NONE;
  const od = p.onDemand;
  if (od?.kind !== "episode" || od.id !== current.episodeId) return NONE;
  const d = od.duration;
  if (d === null || !(d > 0) || od.position < d - UP_NEXT_LEAD_SECS) return NONE;
  const secondsLeft = Math.ceil(d - (UP_NEXT_LEAD_SECS - UP_NEXT_COUNTDOWN_SECS) - od.position);
  return secondsLeft <= 0 ? { kind: "due", next } : { kind: "counting", next, secondsLeft };
}

/** What answers Enter itself when it has focus: a control, a link, a field,
 *  a list's row or option, a tab, anything focusable. Enter there is its. */
export const ENTER_OWNERS =
  "button, a, input, textarea, select, [role=button], [role=option], [role=row], [role=tab], " +
  "[contenteditable], [tabindex]";
/** Where focus rests without a control of its own: the page, the player,
 *  the browser's picture (focusable only to take clicks). */
const NO_OWNER = "body, .chrome, video";

interface KeyTarget {
  closest(selector: string): { matches(selector: string): boolean } | null;
}

/** Whether Enter is Up next's Play now: only while the card shows in the
 *  full player (not docked, not stacked over a list), and only when nothing
 *  else has taken the key — not already handled, and not pressed on
 *  something that answers Enter itself (Pause after a click, a list's row). */
export function upNextTakesEnter(
  e: { key: string; defaultPrevented: boolean; target: unknown },
  cardInFullPlayer: boolean,
): boolean {
  if (e.key !== "Enter" || !cardInFullPlayer || e.defaultPrevented) return false;
  const t = e.target as Partial<KeyTarget> | null;
  if (!t || typeof t.closest !== "function") return true;
  const owner = t.closest(ENTER_OWNERS);
  return owner === null || owner.matches(NO_OWNER);
}

/** Moving on from an episode is claimed once: the countdown, Play now and
 *  Enter may all ask, and only the first is acted on. Opening the episode
 *  again (`reset`) is another playing of it, which may move on again. */
export function createAdvanceGuard() {
  const claimed = new Set<number>();
  return {
    /** True the first time for `from`, false after. */
    claim(from: number): boolean {
      if (claimed.has(from)) return false;
      claimed.add(from);
      return true;
    },
    /** `id` is being opened anew. */
    reset(id: number): void {
      claimed.delete(id);
    },
  };
}
