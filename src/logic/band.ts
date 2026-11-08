// What the band across the top of browse mode is about. It can tell of two
// things at once: what is selected in a list, and what is playing. Pure;
// exercised by src/__checks__/bandLayout.test.mjs.
//
// The band has a main column (poster, title, facts, plot, actions) and a side
// column beside the picture. With a selection that is not what plays, the
// selection leads and the side column says what is playing, with a way to
// swap the two (↩ Show) that lasts until the next selection. With one thing
// to tell of, the side column gives context for it instead: what is on later,
// the next episode, the languages.

export type BandKind = "channel" | "movie" | "show" | "episode";

export interface BandItem {
  kind: BandKind;
  /** The same title whatever is said of it: an episode's key is its show's,
   *  so a show selected while one of its episodes plays is one thing. */
  key: string;
}

/** Which of the two leads while both are told of. */
export type BandLead = "selection" | "playing";

/** What fills the side column with one thing told of. */
export type BandContext = "later" | "next-episode" | "up-next" | "languages";

export interface BandLayout {
  /** What the main column tells of. */
  main: "selection" | "playing" | null;
  /** What the side column tells of: the other one (compact), or context for
   *  the main one. */
  side: "selection" | "playing" | "context" | null;
}

export interface BandInput {
  playing: BandItem | null;
  selected: BandItem | null;
  /** A sheet is open over the list. It carries the selection, so the band
   *  is free for what plays (while something does). */
  sheetOpen: boolean;
  lead: BandLead;
}

export function bandLayout({ playing, selected, sheetOpen, lead }: BandInput): BandLayout {
  const sel = sheetOpen && playing ? null : selected;
  if (playing && sel) {
    if (playing.key === sel.key) return { main: "playing", side: "context" };
    return lead === "playing"
      ? { main: "playing", side: "selection" }
      : { main: "selection", side: "playing" };
  }
  if (sel) return { main: "selection", side: "context" };
  if (playing) return { main: "playing", side: "context" };
  return { main: null, side: null };
}

/** The side column's context for the one thing the band tells of. */
export function bandContext(kind: BandKind): BandContext {
  switch (kind) {
    case "channel":
      return "later";
    case "episode":
      return "next-episode";
    case "show":
      return "up-next";
    case "movie":
      return "languages";
  }
}

/** The lead after a selection (back to the selection) or ↩ Show (the other
 *  one). Swapping lasts until the next selection. */
export function nextBandLead(lead: BandLead, event: "select" | "swap"): BandLead {
  if (event === "select") return "selection";
  return lead === "selection" ? "playing" : "selection";
}
