// The desktop's media controls, the wiring half: telling the backend what is
// on, answering the keys it passes back, and stepping through the channel
// list for next and previous. The decisions are in src/logic/mediaKeys.ts.
//
// Only while Settings › Playback › Media keys is on. The backend does not
// register with the desktop otherwise, so no key arrives; the session is not
// sent either, which keeps an IPC call per channel change away from anyone
// who never turned it on.

import { useEffect, useRef, type Dispatch } from "react";
import { playChannel } from "./actions";
import { backend, subscribeMediaKey } from "./backend";
import { useClock } from "./hooks/useClock";
import { useNowNext } from "./hooks/useNowNext";
import {
  episodeSessionOf,
  indexOfStream,
  intentOf,
  mediaSessionOf,
  movieSessionOf,
  sessionKey,
  stepFrom,
} from "./logic/mediaKeys";
import { playingKind } from "./logic/movies";
import { run, type Action, type State } from "./store";
import type { ChannelDto, MediaKey } from "./types";

// ------------------------------------------------------------ the zap list

/** The list next and previous step through: whatever the channel list is
 *  showing — a category, or search results. */
export interface ZapList {
  rowAt: (i: number) => ChannelDto | undefined;
  /** Rows the list holds, or projects it holds (the list is paged). */
  total: number;
  /** Rows from `start` that the list has not loaded; absent when everything
   *  is already in hand, as search results are. */
  fetch?: (start: number, count: number) => Promise<ChannelDto[]>;
}

let zapList: () => ZapList | null = () => null;

/** Called by the channel list with a reader of its current rows. A reader
 *  rather than the rows, because the list goes on rendering — a new category,
 *  more pages — while hidden behind the player, where its effects do not run
 *  and could not re-register. */
export function registerZapList(read: () => ZapList | null): void {
  zapList = read;
}

/** Plays the channel after (or before) the one on, in the list on screen.
 *  Nothing happens when the channel playing is not in that list, or the list
 *  ends: a media key pressed from across the room should never land somewhere
 *  surprising. Separators are skipped; a row not loaded yet is fetched. */
export async function stepChannel(
  dispatch: Dispatch<Action>,
  playing: number | null,
  dir: 1 | -1,
): Promise<boolean> {
  const list = zapList();
  if (!list || playing === null) return false;
  const from = indexOfStream(list.rowAt, list.total, playing);
  if (from < 0) return false;

  // Rows fetched for this step, so a run of separators past a page edge is
  // one fetch rather than one per row.
  const fetched = new Map<number, ChannelDto>();
  const rowAt = (i: number) => list.rowAt(i) ?? fetched.get(i);
  let at = from;
  // Bounded: every turn either finds a row, ends, or loads one it lacked.
  for (let guard = 0; guard < 64; guard++) {
    const step = stepFrom(rowAt, list.total, at, dir);
    if (step.kind === "end") return false;
    if (step.kind === "found") {
      void playChannel(dispatch, step.channel);
      return true;
    }
    if (!list.fetch) return false;
    const count = 16;
    const start = dir === 1 ? step.index : Math.max(0, step.index - count + 1);
    const rows = await list.fetch(start, count).catch(() => []);
    if (rows.length === 0) return false;
    rows.forEach((r, k) => fetched.set(start + k, r));
    if (!fetched.has(step.index)) return false;
    at = step.index - dir;
  }
  return false;
}

// ----------------------------------------------------------------- the keys

/** What one key does. Kept to this one function so the meaning of a key can
 *  change in one place — previous might one day mean "the channel before",
 *  the way a remote's recall button does, rather than the row above. */
export function handleMediaKey(
  key: MediaKey,
  state: Pick<State, "player" | "nowChannel" | "pendingStreamId">,
  dispatch: Dispatch<Action>,
): void {
  // Next and previous step through channels; with a movie or an episode on
  // there is no channel to step from, and the one last browsed is not what is playing.
  const playing =
    state.player.onDemand !== null
      ? null
      : (state.pendingStreamId ?? state.player.streamId ?? state.nowChannel?.streamId ?? null);
  switch (intentOf(key, state.player.state)) {
    case "togglePause":
      void run(dispatch, "pause", backend.togglePause());
      break;
    case "next":
      void stepChannel(dispatch, playing, 1);
      break;
    case "previous":
      void stepChannel(dispatch, playing, -1);
      break;
    case "stop":
      void run(dispatch, "stop", backend.stop());
      break;
    case null:
      break;
  }
}

/** Sends the session while the setting is on, and answers the keys. */
export function useMediaControls(state: State, dispatch: Dispatch<Action>): void {
  const on = state.settings.mediaControls;
  // A programme ends while the channel stays on; a coarse tick is enough for
  // the desktop's panel to follow.
  const now = useClock(30_000);
  const { now: programme } = useNowNext(on ? state.nowChannel : null, now);
  const kind = playingKind(state.player);
  const movie = kind === "movie" ? state.movies.playing : null;
  const episode = kind === "episode" ? state.shows.playing : null;
  const session = !on
    ? null
    : movie
      ? movieSessionOf(movie, state.player.state)
      : episode
        ? episodeSessionOf(episode, state.player.state)
        : mediaSessionOf(state.nowChannel, programme, state.player.state);
  const key = sessionKey(session);
  const latest = useRef(session);
  latest.current = session;

  useEffect(() => {
    if (!on) return;
    // A failure is not worth a toast: the desktop's panel is a convenience,
    // and the next change sends it again anyway.
    void backend.setMediaSession(latest.current).catch(() => {});
  }, [on, key]);

  const stateRef = useRef(state);
  stateRef.current = state;
  useEffect(() => {
    if (!on) return;
    let unsub: (() => void) | null = null;
    let live = true;
    void subscribeMediaKey((k) => handleMediaKey(k, stateRef.current, dispatch)).then((u) => {
      if (live) unsub = u;
      else u();
    });
    return () => {
      live = false;
      unsub?.();
    };
  }, [on, dispatch]);
}
