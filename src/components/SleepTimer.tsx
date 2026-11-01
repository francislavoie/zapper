// The sleep timer's clock. Draws nothing; mounted once, by App.
//
// Two jobs, both of which need to watch time pass, and so are kept out of
// App itself, which would otherwise re-render the whole tree on every tick:
//
//  - keep the store told which programme a timer could end with, so the
//    menus and the palette can offer "End of this programme" exactly when
//    the guide knows when that is;
//  - while a timer is set, run it: fade the app's volume over the last ten
//    seconds, stop the stream, and put the volume back for next time.
//
// When the timer is turned off mid-fade the volume goes straight back to
// where it was. The order of the volume calls matters — a late step of the
// fade landing after the restore would leave the next channel nearly silent
// — so every call goes through one chain.

import { useEffect, useRef } from "react";
import { backend } from "../backend";
import { useClock } from "../hooks/useClock";
import { useNowNext } from "../hooks/useNowNext";
import { fadedVolume, programmeForSleep, sleepStep } from "../logic/sleep";
import { run, useApp, useDispatch } from "../store";

/** How often a running timer looks at the clock: often enough that the fade
 *  is smooth (fifty steps over ten seconds), and a timer costs nothing until
 *  its last ten seconds, when it has something to do. */
const TICK_MS = 200;

export function SleepTimer() {
  const { nowChannel, player, sleep } = useApp();
  const dispatch = useDispatch();

  // Which programme is on: polled each half minute, like the band's.
  const now = useClock(30_000);
  const { now: onNow } = useNowNext(nowChannel, now);
  const programme = programmeForSleep(onNow, nowChannel?.nowPlaying ?? null, now);
  useEffect(() => {
    dispatch({ type: "sleepProgramme", programme });
    // Keyed on what identifies a programme, not the object: the hook hands
    // back a fresh one on every render.
  }, [programme?.start, programme?.stop, programme?.title, dispatch]);

  // Read inside the interval, which outlives any one render.
  const volume = useRef(player.volume);
  volume.current = player.volume;

  useEffect(() => {
    if (!sleep) return;
    /** The volume when the fade began, and so the one to put back. */
    let from: number | null = null;
    let last: number | null = null;
    let done = false;
    let chain: Promise<unknown> = Promise.resolve();
    const then = (f: () => Promise<unknown>) => {
      chain = chain.then(f, f);
      return chain;
    };

    const id = window.setInterval(() => {
      const step = sleepStep(sleep, Date.now());
      if (step.phase === "waiting") return;
      if (step.phase === "fading") {
        if (from === null) from = volume.current;
        const start = from;
        const v = fadedVolume(start, step.level);
        if (last === null || Math.abs(v - last) >= 0.5) {
          last = v;
          void then(() => backend.setVolume(v).catch(() => {}));
        }
        return;
      }
      // Expired. Cleared first: the stop below would clear it anyway, and the
      // cleanup must know it was this, not a cancellation, that ended it.
      done = true;
      window.clearInterval(id);
      const restore = from ?? volume.current;
      dispatch({ type: "sleep", request: null, at: Date.now() });
      void then(() => run(dispatch, "stop", backend.stop()))
        .then(() => then(() => backend.setVolume(restore).catch(() => {})))
        .then(() => {
          // A stopped player is not a thing to sit looking at, the same
          // rule as Stop itself.
          dispatch({ type: "mode", mode: "browse" });
          dispatch({ type: "toast", text: "Sleep timer: playback stopped", kind: "info" });
        });
    }, TICK_MS);

    return () => {
      window.clearInterval(id);
      // Turned off, or replaced, partway through the fade: the sound comes
      // straight back rather than staying where the fade left it.
      if (!done && from !== null) {
        const restore = from;
        void then(() => backend.setVolume(restore).catch(() => {}));
      }
    };
  }, [sleep, dispatch]);

  return null;
}
