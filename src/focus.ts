// `/` has to reach the channel search box from anywhere, including from over
// the video where no React tree is focused. A one-element registry is less
// machinery than threading a ref through the shell.

let searchEl: HTMLInputElement | null = null;

export function registerSearchInput(el: HTMLInputElement | null) {
  searchEl = el;
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
