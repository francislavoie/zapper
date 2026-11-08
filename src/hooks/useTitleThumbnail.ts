// The preview above a movie's or an episode's seek bar.
//
// Asked once per slot (logic/trickplay.ts) rather than per pixel: the
// backend holds at most one frame a slot, so every position inside one gets
// the same answer, and a sweep across the bar is a few dozen calls. Asked
// again every couple of seconds while the pointer rests, because the slot
// may be filled meanwhile (it was buffered and not yet decoded, or it has
// just played), and afresh for another title (Up next moving on).

import { useEffect, useState } from "react";
import { backend } from "../backend";
import { slotOf, titleSpacing } from "../logic/trickplay";
import type { TitleThumbnailDto } from "../types";

/** How often a resting pointer asks again. */
const REASK_MS = 2000;

/** `at` of null means nothing to preview: the pointer is not on the bar.
 *  `title` names the title (its kind and id), so another one asks anew. */
export function useTitleThumbnail(
  title: string,
  at: number | null,
  duration: number | null,
): TitleThumbnailDto | null {
  const [thumb, setThumb] = useState<TitleThumbnailDto | null>(null);
  const [round, setRound] = useState(0);
  const spacing = titleSpacing(duration);
  const slot = at === null || !Number.isFinite(at) ? null : slotOf(Math.max(0, at), spacing);

  useEffect(() => {
    if (slot === null) return;
    const t = window.setInterval(() => setRound((r) => r + 1), REASK_MS);
    return () => window.clearInterval(t);
  }, [slot, title]);

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
  }, [slot, spacing, title, round]);

  return thumb;
}
