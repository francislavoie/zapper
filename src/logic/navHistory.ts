// Where the mouse's back and forward buttons go once nothing is open: the
// tabs and rail choices the viewer has been through, as a browser's history
// is the pages. In memory only, and short: a way back to where you just
// were, not a record of the session.
//
// Plain data and pure functions; useGlobalKeys holds one and reports each
// place the store moves to.

/** A screen: the tab, and what its rail has chosen. */
export interface Place {
  section: "live" | "movies" | "shows";
  /** The rail's choice on that tab: a category id, or a MOVIE_* / SHOW_* id. */
  rail: string;
}

export interface NavHistory {
  places: readonly Place[];
  /** The place showing now, an index into `places`. */
  at: number;
}

/** How many places are kept. Older ones fall off the start. */
export const NAV_HISTORY_CAP = 20;

export function startHistory(place: Place): NavHistory {
  return { places: [place], at: 0 };
}

export function samePlace(a: Place, b: Place): boolean {
  return a.section === b.section && a.rail === b.rail;
}

/** The store moved to `place`. Where it already is (a back or forward
 *  arriving, or a re-render that changed nothing) nothing changes; anywhere
 *  else is a new step, which drops whatever Forward would have gone to, as a
 *  browser does. */
export function visit(h: NavHistory, place: Place, cap = NAV_HISTORY_CAP): NavHistory {
  const here = h.places[h.at];
  if (here && samePlace(here, place)) return h;
  const places = [...h.places.slice(0, h.at + 1), place];
  const drop = Math.max(0, places.length - cap);
  return { places: places.slice(drop), at: places.length - 1 - drop };
}

/** One step back: the place to go to, and the history once there; null at
 *  the start. Places `reachable` turns down — a tab this account no longer
 *  has — are stepped over. */
export function goBack(
  h: NavHistory,
  reachable: (p: Place) => boolean = () => true,
): { place: Place; history: NavHistory } | null {
  for (let at = h.at - 1; at >= 0; at--) {
    if (reachable(h.places[at])) return { place: h.places[at], history: { places: h.places, at } };
  }
  return null;
}

/** One step forward again, after a back; null with nothing ahead. Steps
 *  over what cannot be reached, as goBack does. */
export function goForward(
  h: NavHistory,
  reachable: (p: Place) => boolean = () => true,
): { place: Place; history: NavHistory } | null {
  for (let at = h.at + 1; at < h.places.length; at++) {
    if (reachable(h.places[at])) return { place: h.places[at], history: { places: h.places, at } };
  }
  return null;
}

/** The mouse's side buttons, as `MouseEvent.button` numbers them. */
export const MOUSE_BACK = 3;
export const MOUSE_FORWARD = 4;

/** A press of either side button. Anything that dismisses or drags on a
 *  press ignores these: the press is the back or forward button's, and a
 *  menu it closed on the way down would be a second step for one press. */
export function isSideButton(e: { button: number }): boolean {
  return e.button === MOUSE_BACK || e.button === MOUSE_FORWARD;
}

/** How long after acting on a press another release of the same button is
 *  taken as the same press arriving by another event. */
export const SAME_PRESS_MS = 400;

/** One press of a side button can arrive as several events — `pointerup`,
 *  `mouseup`, `auxclick`, in an order and a selection that differ between
 *  engines (WebKitGTK and Chromium do not agree). This decides which of them
 *  acts: the first release after each press, and, when an engine sends no
 *  press at all, the first release that is not close behind the last one
 *  acted on.
 *
 *  Returns a function to call with every such event; it says whether this
 *  one is the one to act on. */
export function buttonLatch(): (kind: "down" | "up", button: number, time: number) => boolean {
  let armed: number | null = null;
  let last: { button: number; time: number } | null = null;
  return (kind, button, time) => {
    if (kind === "down") {
      armed = button;
      return false;
    }
    if (armed === button) {
      armed = null;
      last = { button, time };
      return true;
    }
    if (armed === null && (!last || last.button !== button || time - last.time > SAME_PRESS_MS)) {
      last = { button, time };
      return true;
    }
    return false;
  };
}
