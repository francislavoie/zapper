// What one Escape press undoes, and what one press of the mouse's back
// button undoes: the same list, in the same order, so the two can never
// disagree about what is on top.
//
// Kept apart from the DOM and the store so the order can be checked without
// either: useGlobalKeys gathers what is open into an EscapeContext and acts
// on the step this returns.

/** What is open, as far as unwinding it goes. */
export interface EscapeContext {
  programmeMenu: boolean;
  favouriteMenu: boolean;
  categoryMenu: boolean;
  categoryOptions: boolean;
  moviesMenu: boolean;
  showsMenu: boolean;
  /** A Continue watching entry's menu (continueState.ts). */
  continueMenu: boolean;
  videoMenu: boolean;
  menuOpen: boolean;
  aboutOpen: boolean;
  updateOpen: boolean;
  settingsOpen: boolean;
  paletteOpen: boolean;
  /** A movie's Details over the list, showing (not behind the player). */
  movieSheet: boolean;
  /** A show's sheet, the same way. */
  showSheet: boolean;
  /** The episode pane beside the Shows list, showing. Escape from its
   *  episodes goes back to the list first (ShowPane answers that itself);
   *  from anywhere else it closes the pane, the selection staying. */
  showPane: boolean;
  /** The Resume question in the full player. */
  resumePrompt?: boolean;
  /** The Up next card over the player. */
  upNextCard: boolean;
  fullscreen: boolean;
  /** The search box of the tab showing has something in it. */
  query: boolean;
  /** And has focus: Escape there clears it before closing the pane. */
  queryFocused: boolean;
  mode: "browse" | "watch" | "guide";
  /** Something is open in the player (playing, paused, opening). */
  playing: boolean;
}

export type EscapeStep =
  | "programmeMenu"
  | "favouriteMenu"
  | "categoryMenu"
  | "categoryOptions"
  | "moviesMenu"
  | "showsMenu"
  | "continueMenu"
  | "videoMenu"
  | "menu"
  | "about"
  | "update"
  | "settings"
  | "palette"
  | "movieSheet"
  | "showSheet"
  | "showPane"
  | "resumePrompt"
  | "upNextCard"
  | "fullscreen"
  | "query"
  /** The guide or the full player, back to the list. */
  | "browse"
  /** From the list to the playing picture: Escape's last step. */
  | "watch"
  /** Nothing open: the mouse's back button goes to the previous tab or
   *  rail choice (logic/navHistory.ts). */
  | "history"
  | "none";

/** One press, one dismissal, in the order things were stacked up.
 *
 *  `via` is what was pressed. The mouse's back button walks the same list
 *  with three differences, all of them about it being a way back rather
 *  than a way out: it leaves the Up next card alone (Escape cancels it, and
 *  that is a choice about the episode, not a step back), it does not clear a
 *  search (a search is not a screen), and with nothing open it goes to the
 *  screen before rather than into the player. */
export function escapeStep(c: EscapeContext, via: "key" | "mouse"): EscapeStep {
  // The menus first: whichever is open was opened last, since none of them
  // can be opened from behind another.
  if (c.programmeMenu) return "programmeMenu";
  if (c.favouriteMenu) return "favouriteMenu";
  if (c.categoryMenu) return "categoryMenu";
  if (c.categoryOptions) return "categoryOptions";
  if (c.moviesMenu) return "moviesMenu";
  if (c.showsMenu) return "showsMenu";
  if (c.continueMenu) return "continueMenu";
  if (c.videoMenu) return "videoMenu";
  if (c.menuOpen) return "menu";
  if (c.aboutOpen) return "about";
  // Above settings: the settings pane's "Check now" opens it there.
  if (c.updateOpen) return "update";
  if (c.settingsOpen) return "settings";
  if (c.paletteOpen) return "palette";
  if (c.movieSheet) return "movieSheet";
  if (c.showSheet) return "showSheet";
  // The countdown first: stacked, it runs over the list the pane is in.
  if (c.resumePrompt && via === "key") return "resumePrompt";
  if (c.upNextCard && via === "key") return "upNextCard";
  if (c.showPane) {
    if (c.query && c.queryFocused && via === "key") return "query";
    return "showPane";
  }
  if (c.fullscreen) return "fullscreen";
  if (c.query && via === "key") return "query";
  if (c.mode === "guide" || c.mode === "watch") return "browse";
  if (via === "mouse") return "history";
  if (c.playing) return "watch";
  return "none";
}
