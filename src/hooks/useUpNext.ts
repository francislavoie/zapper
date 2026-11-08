// Up next, as the app runs it. Mounted once, by App, so it counts and moves
// on wherever the picture is — the full player, docked beside the lists, or
// under the Movies or Live TV tabs — and hands the count to the cards
// (UpNextCard) and the keys (useGlobalKeys).
//
// The count is the player's position's (logic/upNext.ts); the episodes are
// the playing show's details, which the band or the sheet has usually read
// already. When it is due, the next one is played: the action's guard makes
// a repeat (a re-render while due, a press of Play now at the same moment)
// harmless.

import { useEffect, useMemo } from "react";
import { playNextEpisode } from "../actions";
import { sleepStep } from "../logic/sleep";
import { nextEpisode, upNextState, type UpNext } from "../logic/upNext";
import { useApp, useDispatch } from "../store";
import { useShowDetails } from "./useShowDetails";

export function useUpNext(): UpNext {
  const { player, shows, sleep } = useApp();
  const dispatch = useDispatch();
  const playing = shows.playing;
  const seriesId = playing?.series.seriesId ?? null;
  const details = useShowDetails(
    playing?.series ?? null,
    0,
    seriesId === null ? 0 : (shows.detailsRevision[seriesId] ?? 0),
  );
  const episodes = details.status === "ready" ? details.data.episodes : null;
  const currentId = playing?.episode.episodeId ?? null;
  const next = useMemo(
    () => (episodes && currentId !== null ? nextEpisode(episodes, currentId) : null),
    [episodes, currentId],
  );
  // A timer that has ended playback, or is fading it out, ends the evening:
  // nothing more plays by itself.
  const sleeping = sleep !== null && sleepStep(sleep, Date.now()).phase !== "waiting";
  const upNext = upNextState(player, playing?.episode ?? null, next, shows.upNextCancelled, sleeping);

  const dueNext = upNext.kind === "due" ? upNext.next : null;
  useEffect(() => {
    if (dueNext && playing) void playNextEpisode(dispatch, playing, dueNext);
  }, [dueNext, playing, dispatch]);

  return upNext;
}
