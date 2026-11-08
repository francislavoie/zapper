// The movie or episode open in the player, as the subtitle finder names a
// title: what "Find subtitles…" is offered for.

import { useMemo } from "react";
import { playingKind } from "../logic/movies";
import type { TitleRef } from "../onlineSubtitles";
import { useApp } from "../store";

/** `null` for a channel, or with nothing open. */
export function useOnDemandTitle(): TitleRef | null {
  const { player } = useApp();
  const kind = playingKind(player);
  const id = player.onDemand?.id;
  return useMemo(
    () => ((kind === "movie" || kind === "episode") && id !== undefined ? { kind, id } : null),
    [kind, id],
  );
}
