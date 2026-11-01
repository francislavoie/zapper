// The sleep timer: when it ends, how the sound goes, and how it is said.
//
// The ending is a fade rather than a cut. Ten seconds of the app's own volume
// sliding to nothing, then the stream stops — the way a television with a
// sleep button does it, and quiet enough not to wake anyone the cut would.
// Only the app's volume moves, never the system's, and it is put back
// afterwards so the next channel does not open silent.
//
// Pure: times are passed in, nothing reads a clock. Checked in
// src/__checks__/sleep.test.mjs.

import type { ProgrammeDto } from "../types";

/** The fixed lengths on offer, in minutes. */
export const SLEEP_MINUTES = [15, 30, 45, 60, 90] as const;

/** How long the sound takes to go. */
export const SLEEP_FADE_MS = 10_000;

/** A programme that ends sooner than this is not offered as the end: the
 *  timer would be fading before the menu had closed. */
export const PROGRAMME_MIN_LEAD_S = 60;

/** What was asked for: a number of minutes, or the end of the programme
 *  that is on. */
export type SleepRequest = number | "programme";

export interface SleepTimer {
  request: SleepRequest;
  /** Wall-clock milliseconds. */
  endsAt: number;
  /** The programme it ends with, when it ends with one. */
  programme: string | null;
}

/** A timer for a request made at `nowMs`, or `null` when it cannot be had:
 *  the programme's end is unknown, or too close. */
export function startSleep(
  request: SleepRequest,
  nowMs: number,
  programme: Pick<ProgrammeDto, "stop" | "title"> | null,
): SleepTimer | null {
  if (request === "programme") {
    if (!programme || programme.stop * 1000 - nowMs < PROGRAMME_MIN_LEAD_S * 1000) return null;
    return { request, endsAt: programme.stop * 1000, programme: programme.title };
  }
  if (!(request > 0)) return null;
  return { request, endsAt: nowMs + request * 60_000, programme: null };
}

export function remainingMs(t: SleepTimer, nowMs: number): number {
  return Math.max(0, t.endsAt - nowMs);
}

export type SleepStep =
  | { phase: "waiting" }
  /** `level` runs from 1 at the start of the fade to 0 at the end. */
  | { phase: "fading"; level: number }
  | { phase: "expired" };

export function sleepStep(t: SleepTimer, nowMs: number): SleepStep {
  const left = t.endsAt - nowMs;
  if (left <= 0) return { phase: "expired" };
  if (left > SLEEP_FADE_MS) return { phase: "waiting" };
  return { phase: "fading", level: left / SLEEP_FADE_MS };
}

/** The volume partway through the fade, from the level it started at.
 *
 *  Linear in the slider's own units, which are already perceptual (the
 *  backend maps them onto mpv's cubic curve), so a straight line here is a
 *  steady fade to the ear. To a tenth, because that is the finest step worth
 *  a round trip to the player. */
export function fadedVolume(from: number, level: number): number {
  const l = Math.min(1, Math.max(0, level));
  return Math.round(from * l * 10) / 10;
}

/** How long is left, said briefly: "42s", "30m", "1h05m".
 *
 *  Minutes round up. A timer set for 30 minutes says "30m" for its first
 *  minute rather than "29m" the instant it is set, and "1m" means there is
 *  still a minute, not that there is less than one. */
export function remainingLabel(ms: number): string {
  const s = Math.max(0, Math.ceil(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.ceil(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  return `${h}h${String(m % 60).padStart(2, "0")}m`;
}

/** The choice as a menu says it. */
export function requestLabel(request: SleepRequest): string {
  return request === "programme" ? "End of this programme" : `${request} minutes`;
}

export function isRequest(t: SleepTimer | null, request: SleepRequest): boolean {
  return t !== null && t.request === request;
}

/** The programme whose end the timer may be set to, or `null`.
 *
 *  It has to be on now, by the guide, and far enough from its end. And the
 *  stream has to be live: a catch-up of something that has already aired
 *  is watched on its own clock, which the guide's "now" says nothing about —
 *  and it ends by itself anyway. `watching` is the programme the player was
 *  opened for, when it was opened for one. */
export function programmeForSleep(
  onNow: ProgrammeDto | null,
  watching: ProgrammeDto | null,
  nowSec: number,
): ProgrammeDto | null {
  if (!onNow) return null;
  if (watching && watching.stop <= nowSec) return null;
  if (onNow.start > nowSec) return null;
  if (onNow.stop - nowSec < PROGRAMME_MIN_LEAD_S) return null;
  return onNow;
}
