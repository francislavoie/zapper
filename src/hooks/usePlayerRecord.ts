// What Record from the player would do now: the programme on now on the
// channel playing, or its recording (logic/recordings.ts `playerRecord`).
// Read by the picture's menu, the now-playing panel and the palette.

import { playerRecord, type PlayerRecord } from "../logic/recordings";
import { useApp } from "../store";
import { useClock } from "./useClock";

export function usePlayerRecord(): PlayerRecord | null {
  const { recordings, nowChannel, nowProgramme, player } = useApp();
  const now = useClock(15_000);
  if (player.state === "stopped") return null;
  return playerRecord(recordings, nowChannel, nowProgramme, now);
}
