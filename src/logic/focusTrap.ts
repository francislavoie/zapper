// Tab and Shift+Tab kept inside a modal dialog: past the last control back
// to the first, and before the first round to the last, so the keyboard
// never lands on the page behind it. The step is worked out here, apart
// from the DOM, so it can be checked; hooks/useFocusTrap.ts applies it.
//
// Pure. Checked in src/__checks__/focusTrap.test.mjs.

/** What can take focus with Tab, in a dialog. */
export const TABBABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), ' +
  'select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** Which of `count` controls Tab (or Shift+Tab, `back`) moves to from the
 *  one at `at` (-1: none of them has focus), wrapping at either end. `null`
 *  when there is nothing to move to: focus stays where it is. */
export function trapStep(count: number, at: number, back: boolean): number | null {
  if (count <= 0) return null;
  if (at < 0 || at >= count) return back ? count - 1 : 0;
  return (at + (back ? count - 1 : 1)) % count;
}
