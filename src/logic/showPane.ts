// The Shows tab's episode pane, beside the list: which keys move between the
// list and the pane's episodes, and which season `[` and `]` go to. Pure;
// exercised by src/__checks__/showPane.test.mjs.
//
//   list      ↑ ↓ move the selection (the pane follows); Enter or → go into
//             the episodes, opening the pane if it is closed. Esc there is
//             the Escape chain's (logic/escape.ts), which closes the pane.
//   episodes  ↑ ↓ move the cursor; Enter plays; [ ] change season; ← or Esc
//             go back to the list, the pane staying open.

/** Where the keys are: the list with the pane closed or open, or the pane's
 *  episodes. */
export type PaneZone = "closed" | "list" | "episodes";

export interface PaneMove {
  zone: PaneZone;
  /** Enter on an episode. */
  play?: true;
  /** `[` or `]`. */
  season?: -1 | 1;
}

/** What `key` does from `zone`; `null` when it is not the pane's (the list's
 *  own arrows, the player's keys). `selected`: a show is selected, so there
 *  is a pane to go into. `posters`: the list is the poster grid, where →
 *  moves along a row rather than into the pane. */
export function paneKey(
  zone: PaneZone,
  key: string,
  o: { selected: boolean; posters: boolean },
): PaneMove | null {
  if (zone === "episodes") {
    switch (key) {
      case "Enter":
        return { zone, play: true };
      case "[":
        return { zone, season: -1 };
      case "]":
        return { zone, season: 1 };
      case "ArrowLeft":
      case "Escape":
        return { zone: "list" };
      case "ArrowRight":
        // Nowhere further right; taken so it does not seek the player.
        return { zone };
      default:
        return null;
    }
  }
  if (key === "Enter" || (key === "ArrowRight" && !o.posters)) {
    return o.selected ? { zone: "episodes" } : null;
  }
  return null;
}

/** The season `delta` along from `current` in the show's order, held at
 *  either end; `null` when the show has no seasons. */
export function stepSeason(seasons: readonly number[], current: number | null, delta: -1 | 1): number | null {
  if (seasons.length === 0) return null;
  const at = current === null ? -1 : seasons.indexOf(current);
  if (at < 0) return seasons[0];
  return seasons[Math.max(0, Math.min(seasons.length - 1, at + delta))];
}
