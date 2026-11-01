// When controls over the picture are on screen: the state machine in
// logic/fade.ts, and the clock it needs. Shared by the player's chrome and
// the docked picture's controls, so both fade on the same rules.

import { useCallback, useEffect, useState } from "react";
import { initialFade, nextFade, type FadeEvent, type FadeState } from "../logic/fade";

/** `startVisible` is false for the docked picture: its controls appear when
 *  the pointer arrives, not when a stream starts with the pointer elsewhere —
 *  that would be five seconds of a panel over a picture nobody is pointing
 *  at. The player starts visible, because opening it is pointing at it. */
export function useFade(startVisible = true): [FadeState, (e: FadeEvent) => void] {
  const [fade, setFade] = useState<FadeState>(() => {
    const s = initialFade(performance.now());
    return startVisible ? s : { ...s, visible: false };
  });

  const on = useCallback(
    (e: FadeEvent) => setFade((s) => nextFade(s, e, performance.now())),
    [],
  );

  // The idle countdown needs a clock of its own; the state machine only
  // decides, it does not keep time.
  useEffect(() => {
    const t = window.setInterval(() => on({ type: "tick" }), 500);
    return () => window.clearInterval(t);
  }, [on]);

  return [fade, on];
}
