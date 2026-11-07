// Other sources for a channel's programmes: "Try another source:" under the
// message for a channel that is off air.
//
// Large catalogues carry the same channel several times over (an HD and an
// SD feed, another region's copy), and the guide gives them the same id.
// When one of them is off air, another may well be broadcasting. Which are
// offered is decided by the backend (`store::alternatives_for` in Rust, an
// indexed query); `rankAlternatives` is the same rule over a list in memory,
// for the mock and anything else that holds the channels itself. Pure, and
// checked in src/__checks__/alternatives.test.mjs.

import { qualityBadge } from "./format.ts";
import type { ChannelDto } from "../types.ts";

/** The most offered. */
export const ALTERNATIVES_MAX = 3;

/** Played before: a picture came up the last time anyone tried. */
function played(c: ChannelDto): boolean {
  return (c.quality?.okCount ?? 0) > 0;
}

/** Which of `candidates` (channels with the same guide id) to offer instead
 *  of `streamId`, in order: those that last played OK, the most recently
 *  measured first, then the rest by number. Itself, dividers, and channels
 *  whose last open failed or found them off air are left out. At most
 *  `max`. */
export function rankAlternatives(
  candidates: readonly ChannelDto[],
  streamId: number,
  max: number = ALTERNATIVES_MAX,
): ChannelDto[] {
  return candidates
    .filter((c) => c.streamId !== streamId && !c.isSeparator && c.quality?.failedAt == null)
    .sort((a, b) => {
      const pa = played(a);
      const pb = played(b);
      if (pa !== pb) return pa ? -1 : 1;
      if (pa) {
        const d = (b.quality?.measuredAt ?? 0) - (a.quality?.measuredAt ?? 0);
        if (d !== 0) return d;
      }
      if (a.num !== b.num) {
        if (a.num == null) return 1;
        if (b.num == null) return -1;
        return a.num - b.num;
      }
      return a.streamId - b.streamId;
    })
    .slice(0, max);
}

/** One offered channel as its row shows it: number, name, and the
 *  resolution last measured when there is one. */
export interface AlternativeLine {
  num: string;
  name: string;
  /** The full name, for the tip when the row cuts it short. */
  fullName: string;
  resolution: string | null;
}

export function alternativeLine(c: ChannelDto): AlternativeLine {
  return {
    num: c.numLabel ?? (c.num != null ? String(c.num) : ""),
    name: c.cleanName || c.name,
    fullName: c.name,
    resolution: qualityBadge(c.quality),
  };
}
