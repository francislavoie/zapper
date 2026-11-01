// What the corner of the window says, and how much of it at once.
//
// Checked in src/__checks__/toasts.test.mjs.

export interface Toast {
  id: number;
  text: string;
  kind: "error" | "info";
}

/** The most toasts shown at once; a new one pushes out the oldest. */
export const MAX_TOASTS = 3;

/** `toasts` with `next` added, unless the same thing is already being said,
 *  when it is `toasts` itself (so nothing re-renders, and the toast already up
 *  goes away when it was going to).
 *
 *  A double-click is two clicks and a double-click, and each can say the
 *  same thing ("Watching in the browser isn't available yet"); a retry that
 *  fails the same way says it again. The second copy tells nobody anything,
 *  and three of them fill the corner, so a toast already on screen with the
 *  same words and kind is left to stand for all of them. */
export function addToast(toasts: readonly Toast[], next: Toast): readonly Toast[] {
  if (toasts.some((t) => t.text === next.text && t.kind === next.kind)) return toasts;
  return [...toasts.slice(-(MAX_TOASTS - 1)), next];
}
