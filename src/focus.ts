// `/` has to reach the channel search box from anywhere, including from over
// the video where no React tree is focused. A one-element registry is less
// machinery than threading a ref through the shell.

let searchEl: HTMLInputElement | null = null;

/** A ref callback for one list's search box. Two lists (channels and movies)
 *  keep a box each, and the one on screen is the one `/` reaches. Whichever
 *  order they change hands in when the view switches, a box leaving only
 *  clears the registry if it is still the one registered — otherwise the
 *  newcomer's registration would be wiped by the leaver's. */
export function searchInputRegistration(): (el: HTMLInputElement | null) => void {
  let mine: HTMLInputElement | null = null;
  return (el) => {
    if (el) {
      mine = el;
      searchEl = el;
    } else {
      if (searchEl === mine) searchEl = null;
      mine = null;
    }
  };
}

export function focusSearch() {
  searchEl?.focus();
  searchEl?.select();
}

/** True when the keystroke belongs to a text field and the global shortcuts
 *  should keep their hands off it. */
export function typingInto(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el) return false;
  const tag = el.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || el.isContentEditable;
}
