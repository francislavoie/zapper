// A number that glides toward its target instead of jumping to it.
//
// The guide's horizontal position, mostly: the store says where the guide is
// going and this says where it has got to, frame by frame. The arithmetic is
// easeToward() in logic/guide.ts; this is only the requestAnimationFrame loop
// around it.
//
// The loop reads the target through a ref, so a target that moves while it is
// running — the next wheel notch — is chased from wherever the value has got
// to, with nothing restarted. The value lives here, in component state, not in
// the store: only the component drawing it re-renders per frame, and nothing
// is dispatched per frame.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { GUIDE_EASE_MS, easeToward } from "../logic/guide";

/** `snap` is how close counts as arrived, in the target's units. `jump`
 *  skips the easing altogether — for a drag, where the value must stay under
 *  the pointer — and the value is the target for as long as it is set. */
export function useEasedTime(target: number, snap: number, jump: boolean): number {
  const [shown, setShown] = useState(target);
  const shownRef = useRef(target);
  const targetRef = useRef(target);
  const snapRef = useRef(snap);
  targetRef.current = target;
  snapRef.current = snap;
  const frame = useRef(0);

  // A layout effect, so the frame that ends a drag (jump going false) is
  // corrected before it is painted rather than flashing the pre-drag value.
  useLayoutEffect(() => {
    if (jump) {
      cancelAnimationFrame(frame.current);
      frame.current = 0;
      shownRef.current = target;
      return;
    }
    if (frame.current) return;
    if (shownRef.current === target) {
      setShown(target);
      return;
    }
    let last = performance.now();
    const step = (t: number) => {
      const next = easeToward(shownRef.current, targetRef.current, t - last, GUIDE_EASE_MS, snapRef.current);
      last = t;
      shownRef.current = next;
      setShown(next);
      frame.current = next === targetRef.current ? 0 : requestAnimationFrame(step);
    };
    frame.current = requestAnimationFrame(step);
  }, [target, jump]);

  // Hidden (an <Activity> unmounts effects) or gone: stop. Shown again, the
  // effect above picks up from wherever the value was left.
  useEffect(
    () => () => {
      cancelAnimationFrame(frame.current);
      frame.current = 0;
    },
    [],
  );

  return jump ? target : shown;
}
