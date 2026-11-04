// The desktop's media controls, the decisions half: what they are told, what
// each key does, and which channel "next" is. The wiring is src/mediaKeys.ts.
// Pure; checked in src/__checks__/mediaKeys.test.mjs.

import type {
  ChannelDto,
  MediaKey,
  MediaPlayback,
  MediaSessionDto,
  PlaybackState,
  ProgrammeDto,
} from "../types";
import { movieFacts } from "./movies.ts";

/** How the player's state reads to the desktop. Opening counts as playing,
 *  since that is what was asked for and it is a second away; a failed open
 *  has nothing on screen, so it is stopped. */
export function playbackOf(state: PlaybackState): MediaPlayback {
  switch (state) {
    case "playing":
    case "opening":
      return "playing";
    case "paused":
      return "paused";
    case "stopped":
    case "failed":
      return "stopped";
  }
}

/** What the desktop shows: the channel as the title, the programme beneath,
 *  the logo as artwork. Null with nothing on, which clears the panel. */
export function mediaSessionOf(
  channel: ChannelDto | null,
  programme: ProgrammeDto | null,
  state: PlaybackState,
): MediaSessionDto | null {
  if (!channel || state === "stopped") return null;
  const title = (channel.cleanName || channel.name).trim();
  const subtitle = programme?.title.trim() || null;
  return {
    title,
    subtitle,
    artUrl: channel.logoUrl || null,
    playback: playbackOf(state),
  };
}

/** The same for a movie: its title, its year and length beneath. The poster
 *  is the service's to serve, not a URL the desktop could fetch, so no art. */
export function movieSessionOf(
  movie: { cleanName: string; name: string; year: number | null; durationSecs: number | null } | null,
  state: PlaybackState,
): MediaSessionDto | null {
  if (!movie || state === "stopped") return null;
  const facts = movieFacts({ year: movie.year, durationSecs: movie.durationSecs, rating: null });
  return {
    title: (movie.cleanName || movie.name).trim(),
    subtitle: facts || null,
    artUrl: null,
    playback: playbackOf(state),
  };
}

/** A string that changes exactly when the session does, so the same thing is
 *  not sent across the boundary on every render. */
export function sessionKey(s: MediaSessionDto | null): string {
  return s === null ? "" : JSON.stringify([s.title, s.subtitle, s.artUrl, s.playback]);
}

/** What a key asks of the app. */
export type MediaIntent = "togglePause" | "next" | "previous" | "stop";

/** Play and pause are not toggles: a desktop that sends "pause" to a paused
 *  player must not start it again. Next and previous mean something only with
 *  a channel to step from, which the stepping itself checks. */
export function intentOf(key: MediaKey, state: PlaybackState): MediaIntent | null {
  const live = state === "playing" || state === "opening";
  switch (key) {
    case "toggle":
      return live || state === "paused" ? "togglePause" : null;
    case "play":
      return state === "paused" ? "togglePause" : null;
    case "pause":
      return live ? "togglePause" : null;
    case "next":
      return "next";
    case "previous":
      return "previous";
    case "stop":
      return state === "stopped" ? null : "stop";
  }
}

// ------------------------------------------------------------ the zap list

/** Where `streamId` is among the rows the list has loaded, or -1. Only loaded
 *  rows are looked at: the list is paged, and the channel playing is almost
 *  always one on a page already fetched, because it was picked from there. */
export function indexOfStream(
  rowAt: (i: number) => ChannelDto | undefined,
  total: number,
  streamId: number,
): number {
  for (let i = 0; i < total; i++) {
    if (rowAt(i)?.streamId === streamId) return i;
  }
  return -1;
}

export type Step =
  | { kind: "found"; index: number; channel: ChannelDto }
  /** The row at `index` has not been loaded; fetch it and step again. */
  | { kind: "missing"; index: number }
  /** The list ends in this direction. No wrapping: the far end of a long
   *  category is not where anyone pressing "next" meant to go. */
  | { kind: "end" };

/** The next playable row after `from` in direction `dir`, skipping the
 *  provider's separators, which are headers and never play. */
export function stepFrom(
  rowAt: (i: number) => ChannelDto | undefined,
  total: number,
  from: number,
  dir: 1 | -1,
): Step {
  for (let i = from + dir; i >= 0 && i < total; i += dir) {
    const row = rowAt(i);
    if (row === undefined) return { kind: "missing", index: i };
    if (!row.isSeparator) return { kind: "found", index: i, channel: row };
  }
  return { kind: "end" };
}
