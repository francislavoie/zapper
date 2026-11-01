// The sleep timer as the menus see it: which of its actions apply, and how
// long is left.

import { availableActions, type PaletteAction } from "../logic/palette";
import { remainingLabel } from "../logic/sleep";
import { useApp } from "../store";
import { useClock } from "./useClock";

/** The sleep actions that apply now: none while nothing is playing. */
export function useSleepActions(): PaletteAction[] {
  const { player, sleep, sleepProgramme } = useApp();
  return availableActions(player.state !== "stopped", {
    sleeping: sleep !== null,
    programme: sleepProgramme !== null,
  }).filter((a) => a.group === "sleep");
}

/** How long is left, ticking, or `null` with no timer set. */
export function useSleepLeft(): string | null {
  const { sleep } = useApp();
  // The clock is only what re-renders this each second. The reading is
  // taken here, to the millisecond: the clock's whole seconds would make a
  // timer set a moment ago read a minute longer than it was set for.
  useClock(1000);
  return sleep ? remainingLabel(sleep.endsAt - Date.now()) : null;
}
