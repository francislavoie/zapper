// How much room the stage has for the docked picture, kept current.
//
// The picture's chosen width is remembered in rem, and a window can open
// smaller than the one it was chosen in. Measured here and handed to the
// store as a ceiling, so the band draws what fits without the remembered
// size being touched — see `videoRoomRem` in src/logic/videoSize.ts.
//
// Measured in JS, not written as a CSS `min()`: the stylesheet may not use
// viewport units (src/__checks__/viewport-units.test.mjs), and the stage's
// size in rem needs a rem measured in the same space as the stage. A hidden
// 10rem probe inside the stage gives exactly that, under either zoom
// mechanism, without knowing which one is in use.

import { useLayoutEffect, type RefObject } from "react";
import { videoRoomRem } from "../logic/videoSize";
import type { Dispatch } from "react";
import type { Action } from "../store";

const PROBE_REM = 10;

export function useVideoRoom(
  stageRef: RefObject<HTMLElement | null>,
  zoom: number,
  dispatch: Dispatch<Action>,
): void {
  useLayoutEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;

    const probe = document.createElement("div");
    probe.setAttribute("aria-hidden", "true");
    probe.style.cssText = `position:absolute;visibility:hidden;pointer-events:none;width:${PROBE_REM}rem;height:0;top:0;left:0`;
    stage.appendChild(probe);

    const measure = () => {
      const pxPerRem = probe.getBoundingClientRect().width / PROBE_REM;
      const box = stage.getBoundingClientRect();
      if (!(pxPerRem > 0)) return;
      dispatch({ type: "videoRoom", rem: videoRoomRem(box.width / pxPerRem, box.height / pxPerRem) });
    };

    // Once now, before paint, so a window opened smaller than last time never
    // shows a frame of the oversized picture; then on every resize.
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(stage);
    return () => {
      ro.disconnect();
      probe.remove();
    };
    // Zoom changes the rem's size in pixels without resizing the stage, so it
    // has to trigger a re-measure of its own.
  }, [stageRef, zoom, dispatch]);
}
