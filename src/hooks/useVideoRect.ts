// Drives where mpv paints. Browsing docks the video into a corner box by
// animating the rect frame by frame; leaving browse restores {0,0,1,1}.
// Nothing is torn down on a mode change — no stop(), no reload, the stream
// just changes shape.

import { useEffect, useRef, type RefObject } from "react";
import { backend } from "../backend";
import {
  FULL,
  fitAspect,
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
  docked: boolean;
  /** Mock-only stand-in painted where video would be, moved without React. */
  paintRef?: RefObject<HTMLElement | null>;
  onError?: (message: string) => void;
}

export function useVideoRect({ dockRef, docked, paintRef, onError }: VideoRectOptions) {
  const current = useRef<VideoRect>(FULL);
  const from = useRef<VideoRect>(FULL);
  const to = useRef<VideoRect>(FULL);
  const startedAt = useRef(0);
  const duration = useRef(0);
  const raf = useRef<number | undefined>(undefined);
  /** Rust has been told where the video goes at least once. */
  const primed = useRef(false);
  const complained = useRef(false);
  const errorSink = useRef(onError);
  errorSink.current = onError;

  useEffect(() => {
    const measure = (): VideoRect => {
      const el = dockRef.current;
      if (!docked || !el) return FULL;
      const b = el.getBoundingClientRect();
      if (b.width <= 0 || b.height <= 0) return FULL;
      const fitted = fitAspect(
        { left: b.left, top: b.top, width: b.width, height: b.height },
        ASPECT,
      );
      return normalisedRect(fitted, window.innerWidth, window.innerHeight);
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
      void backend.setVideoRect(r).catch((e: unknown) => {
        // The rect is sent every frame of a transition; one failure means all
        // of them fail. Say so once, then stay quiet.
        if (complained.current) return;
        complained.current = true;
        errorSink.current?.(typeof e === "string" ? e : String(e));
      });
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

    if (!primed.current) {
      // Boot: snap. There is nothing on screen yet to animate, and Rust needs
      // to be told where the video goes before anything opens.
      primed.current = true;
      send(measure());
    } else {
      // A mode change animates; a resize must not — the window is already
      // moving and a 240ms lag would smear the video behind its hole.
      animateTo(measure(), DOCK_MS);
    }

    const snap = () => animateTo(measure(), 0);
    window.addEventListener("resize", snap);
    const ro =
      typeof ResizeObserver !== "undefined" && dockRef.current
        ? new ResizeObserver(snap)
        : null;
    if (ro && dockRef.current) ro.observe(dockRef.current);

    return () => {
      window.removeEventListener("resize", snap);
      ro?.disconnect();
      if (raf.current !== undefined) cancelAnimationFrame(raf.current);
      raf.current = undefined;
    };
  }, [docked, dockRef, paintRef]);

  return current;
}
