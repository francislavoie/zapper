// The preview above a movie's or an episode's seek bar.
//
// Asked once per slot (logic/trickplay.ts) rather than per pixel: the
// backend holds at most one frame a slot, so every position inside one gets
// the same answer, and a sweep across the bar is a few dozen calls.

import { useEffect, useState } from "react";
import { backend } from "../backend";
import { slotOf, titleSpacing } from "../logic/trickplay";
import type { TitleThumbnailDto } from "../types";

/** `at` of null means nothing to preview: the pointer is not on the bar. */
export function useTitleThumbnail(
  at: number | null,
  duration: number | null,
): TitleThumbnailDto | null {
  const [thumb, setThumb] = useState<TitleThumbnailDto | null>(null);
  const spacing = titleSpacing(duration);
  const slot = at === null || !Number.isFinite(at) ? null : slotOf(Math.max(0, at), spacing);

  useEffect(() => {
    if (slot === null) {
      setThumb(null);
      return;
    }
    // An answer that arrives after the pointer has moved to another slot
    // would show the wrong moment: dropped rather than shown late.
    let live = true;
    void backend
      .thumbnailForTitle(slot * spacing)
      .then((t) => {
        if (live) setThumb(t);
      })
      .catch(() => {
        // No preview is an ordinary answer, not an error worth a toast.
        if (live) setThumb(null);
      });
    return () => {
      live = false;
    };
  }, [slot, spacing]);

  return thumb;
}
