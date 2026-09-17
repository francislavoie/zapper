// The lifetime of a gesture bubble.
//
// It lives here rather than beside the component it drives because a module
// that exports a component *and* something else cannot be hot-reloaded:
// React Fast Refresh needs a module whose every export is a component, and
// one stray hook demotes the whole file to a full page reload on every edit.
// src/__checks__/fast-refresh.test.mjs keeps it that way.

import { useEffect, useState } from "react";
import type { VideoZone } from "../logic/gestures";

export type FeedbackKind = "volume" | VideoZone;

export interface Feedback {
  /** Bumped on every gesture, including a repeat of the same one. */
  id: number;
  kind: FeedbackKind;
  label: string;
  /** Volume only: which speaker glyph to draw. */
  level?: "muted" | "low" | "high";
  /** Seek only: the number the arrow is labelled with. */
  seconds?: number;
}

/** How long a bubble stays mounted. Must outlast the CSS animation, and is
 *  the reason a finished bubble does not keep a stale number on screen. */
const LIFETIME_MS = 900;

/** Drives the bubble's lifetime. Returns what to draw, or null.
 *
 *  Separate from the caller's own state so a gesture does not have to
 *  remember to clear itself, and so a rapid run of gestures keeps extending
 *  one visible bubble rather than stacking timers that clear it early. */
export function useFeedback(feedback: Feedback | null): Feedback | null {
  const [shown, setShown] = useState<Feedback | null>(null);

  useEffect(() => {
    if (!feedback) return;
    setShown(feedback);
    const t = window.setTimeout(() => setShown(null), LIFETIME_MS);
    return () => window.clearTimeout(t);
  }, [feedback]);

  return shown;
}
