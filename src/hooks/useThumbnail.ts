// The scrollback preview under the scrubber's cursor.
//
// Requests are bucketed rather than made per pixel. The pointer moves a pixel
// at a time and the backend only holds a frame every few seconds, so asking
// on every pointermove would be hundreds of round trips that return the same
// picture. The bucket is half the capture interval, which is fine enough that
// no captured frame is unreachable and coarse enough that a sweep across the
// bar costs a handful of calls.

import { useEffect, useState } from "react";
import { backend } from "../backend";
import type { ThumbnailDto } from "../types";

/** Seconds per request bucket. Half the backend's capture interval, so every
 *  frame it holds can still be asked for. */
export const THUMB_BUCKET_S = 2;

/** `behindLive` of null means "nothing to preview" — the pointer has left the
 *  bar, or it is over the archive, which was never on this screen to be
 *  photographed. */
export function useThumbnail(behindLive: number | null): ThumbnailDto | null {
  const [thumb, setThumb] = useState<ThumbnailDto | null>(null);

  const bucket =
    behindLive === null || !Number.isFinite(behindLive)
      ? null
      : Math.max(0, Math.round(behindLive / THUMB_BUCKET_S));

  useEffect(() => {
    if (bucket === null) {
      setThumb(null);
      return;
    }
    // The pointer can outrun a round trip, and an answer that arrives after
    // the cursor has moved on would show the wrong moment. Dropped rather
    // than shown late.
    let live = true;
    void backend
      .thumbnailAt(bucket * THUMB_BUCKET_S)
      .then((t) => {
        if (live) setThumb(t);
      })
      .catch(() => {
        // No preview is an ordinary outcome, not an error worth a toast.
        if (live) setThumb(null);
      });
    return () => {
      live = false;
    };
  }, [bucket]);

  return thumb;
}
