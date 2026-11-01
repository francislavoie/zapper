// Drives where mpv paints. Browsing docks the video into a corner box by
// animating the rect frame by frame; leaving browse restores {0,0,1,1}.
// Nothing is torn down on a mode change — no stop(), no reload, the stream
// just changes shape.

import { useEffect, useRef, useState, type RefObject } from "react";
import { backend } from "../backend";
import {
  FULL,
  fitAspect,
  latestOnly,
  normalisedRect,
  rectAt,
  rectEquals,
  shouldSend,
} from "../logic/rect";
import type { VideoRect } from "../types";

const DOCK_MS = 240;
const ASPECT = 16 / 9;

export interface VideoRectOptions {
  /** The transparent hole the video should fill while docked. */
  dockRef: RefObject<HTMLElement | null>;
  /** Everything below the toolbar: where the video goes when it is not docked
   *  into a corner. Without it, undocked means the whole window — but the app
   *  draws its own opaque title bar, and video behind that bar is video
   *  nobody sees. Measured, never arithmetic on a hard-coded bar height. */
  stageRef?: RefObject<HTMLElement | null>;
  docked: boolean;
  /** Whether the window is fullscreen. Like `docked` this is a change the
   *  picture should travel through rather than jump: entering and leaving
   *  are the same move in opposite directions and should look like it. */
  fullscreen?: boolean;
  /** Application zoom. Not used in the arithmetic — every box here is
   *  measured, never computed — but every box *moves* when it changes, so the
   *  hook has to re-measure and re-send. Without this the picture stays where
   *  the old layout put it and detaches from its slot, which looks like a
   *  video bug and is a zoom bug. */
  zoom?: number;
  /** How wide the docked slot is, in rem. Not used in the arithmetic either
   *  — every box here is measured — but the slot moves when it changes, so
   *  the hook has to re-measure and re-send. */
  videoWidthRem?: number;
  /** Mock-only stand-in painted where video would be, moved without React. */
  paintRef?: RefObject<HTMLElement | null>;
  onError?: (message: string) => void;
}

export function useVideoRect({
  dockRef,
  stageRef,
  docked,
  fullscreen = false,
  zoom,
  videoWidthRem,
  paintRef,
  onError,
}: VideoRectOptions) {
  const current = useRef<VideoRect>(FULL);
  const from = useRef<VideoRect>(FULL);
  const to = useRef<VideoRect>(FULL);
  const startedAt = useRef(0);
  const duration = useRef(0);
  const raf = useRef<number | undefined>(undefined);
  /** Rust has been told where the video goes at least once. */
  const primed = useRef(false);
  /** What the layout was last time round, so a re-run can tell a change of
   *  shape — which animates — from a re-measure, which must not. */
  const wasLayout = useRef(`${docked}|${fullscreen}`);
  const complained = useRef(false);
  const errorSink = useRef(onError);
  errorSink.current = onError;
  // One for the hook's lifetime, not one per effect run: a transition
  // outlives the run that started it, and its frames must queue behind the
  // same send as everything else.
  const [post] = useState(() =>
    latestOnly((r: VideoRect) =>
      backend.setVideoRect(r).catch((e: unknown) => {
        // The rect is sent every frame of a transition; one failure means
        // all of them fail. Say so once, then stay quiet.
        if (complained.current) return;
        complained.current = true;
        errorSink.current?.(typeof e === "string" ? e : String(e));
      }),
    ),
  );

  // The animation is stopped only when the hook goes away. An effect re-run
  // must not stop it: see `remeasure` below.
  useEffect(
    () => () => {
      if (raf.current !== undefined) cancelAnimationFrame(raf.current);
      raf.current = undefined;
    },
    [],
  );

  const layout = `${docked}|${fullscreen}`;

  useEffect(() => {
    const boxOf = (el: HTMLElement | null) => {
      if (!el) return null;
      const b = el.getBoundingClientRect();
      if (b.width <= 0 || b.height <= 0) return null;
      return { left: b.left, top: b.top, width: b.width, height: b.height };
    };

    /** Undocked: the whole stage, unfitted — mpv letterboxes inside whatever
     *  it is given, and a 16:9 fit here would leave bars the app had drawn
     *  rather than bars the player drew. Docked: the slot, fitted, because the
     *  transparent hole has to be the shape of the picture that will land in
     *  it. */
    const measure = (): VideoRect => {
      const stage = boxOf(stageRef?.current ?? null);
      if (!docked) {
        return stage ? normalisedRect(stage, window.innerWidth, window.innerHeight) : FULL;
      }
      const dock = boxOf(dockRef.current);
      if (!dock) {
        return stage ? normalisedRect(stage, window.innerWidth, window.innerHeight) : FULL;
      }
      return normalisedRect(fitAspect(dock, ASPECT), window.innerWidth, window.innerHeight);
    };

    const paint = (r: VideoRect) => {
      const el = paintRef?.current;
      if (!el) return;
      el.style.left = `${r.x * 100}%`;
      el.style.top = `${r.y * 100}%`;
      el.style.width = `${r.w * 100}%`;
      el.style.height = `${r.h * 100}%`;
    };

    const send = (r: VideoRect) => {
      current.current = r;
      paint(r);
      post(r);
    };

    const tick = () => {
      const elapsed = performance.now() - startedAt.current;
      const { rect, done } = rectAt(from.current, to.current, elapsed, duration.current);
      if (shouldSend(current.current, rect, done)) send(rect);
      if (done) {
        raf.current = undefined;
        return;
      }
      raf.current = requestAnimationFrame(tick);
    };

    const animateTo = (target: VideoRect, ms: number) => {
      const settled = rectEquals(current.current, target) && raf.current === undefined;
      if (settled) return;
      from.current = current.current;
      to.current = target;
      startedAt.current = performance.now();
      duration.current = ms;
      if (raf.current === undefined) raf.current = requestAnimationFrame(tick);
    };

    /** The boxes moved without the layout changing shape: a zoom, a window
     *  resize, the picture being resized. Everything on screen moved at
     *  once, so at rest the picture snaps to follow — an animation would be
     *  240ms of video hanging outside its slot.
     *
     *  Mid-transition it is the destination that moved, so the transition
     *  is steered there and carries on at its own pace. Snapping instead is
     *  what made leaving the player instant: the docked slot becomes
     *  visible on the way out, a ResizeObserver reports its first size on
     *  the next frame, and that report cut the move short after one frame. */
    const remeasure = () => {
      const target = measure();
      if (raf.current !== undefined) {
        to.current = target;
        return;
      }
      animateTo(target, 0);
    };

    if (!primed.current) {
      // Boot: snap. There is nothing on screen yet to animate, and Rust needs
      // to be told where the video goes before anything opens.
      primed.current = true;
      send(measure());
    } else if (wasLayout.current !== layout) {
      // A change of shape animates; a plain resize must not — the window is
      // already moving and a 240ms lag would smear the video behind its hole.
      animateTo(measure(), DOCK_MS);
    } else {
      remeasure();
    }
    wasLayout.current = layout;

    window.addEventListener("resize", remeasure);
    const ro =
      typeof ResizeObserver !== "undefined" && dockRef.current
        ? new ResizeObserver(remeasure)
        : null;
    if (ro && dockRef.current) ro.observe(dockRef.current);

    return () => {
      window.removeEventListener("resize", remeasure);
      ro?.disconnect();
    };
    // `zoom` is in the dependency list precisely because it is not used above:
    // it changes every measurement this effect takes. `layout` carries both
    // `docked` and `fullscreen`, and is what decides animate versus snap.
  }, [docked, layout, zoom, videoWidthRem, dockRef, stageRef, paintRef, post]);

  return current;
}
