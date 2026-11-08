// Keeps Tab inside a modal dialog (logic/focusTrap.ts). Returns the key
// handler for the dialog's element; any other key is left alone.

import { useCallback, type KeyboardEvent, type RefObject } from "react";
import { TABBABLE, trapStep } from "../logic/focusTrap";

export function useFocusTrap(
  ref: RefObject<HTMLElement | null>,
): (e: KeyboardEvent<HTMLElement>) => void {
  return useCallback(
    (e: KeyboardEvent<HTMLElement>) => {
      if (e.key !== "Tab" || e.altKey || e.ctrlKey || e.metaKey) return;
      const box = ref.current;
      if (!box) return;
      // Only what is shown: a control in a collapsed part has no box.
      const all = [...box.querySelectorAll<HTMLElement>(TABBABLE)].filter(
        (el) => el.getClientRects().length > 0,
      );
      e.preventDefault();
      const next = trapStep(all.length, all.indexOf(document.activeElement as HTMLElement), e.shiftKey);
      if (next === null) box.focus();
      else all[next]?.focus();
    },
    [ref],
  );
}
