// The macOS app menu's items that the interface carries out: zapper ›
// Settings… (⌘,) and About zapper. The menu itself is Rust's
// (crates/zapper/src/chrome.rs); Quit there closes the window directly, and the
// Edit menu's items are the system's. Elsewhere there is no app menu and this
// never hears anything.

import { useEffect, type Dispatch } from "react";
import { subscribeMenu } from "../backend";
import type { Action } from "../store";

export function useMenuCommands(dispatch: Dispatch<Action>): void {
  useEffect(() => {
    let unsub: (() => void) | null = null;
    let live = true;
    void subscribeMenu((c) => {
      if (c === "settings") dispatch({ type: "settings", open: true });
      else if (c === "about") dispatch({ type: "about", open: true });
    }).then((u) => {
      if (live) unsub = u;
      else u();
    });
    return () => {
      live = false;
      unsub?.();
    };
  }, [dispatch]);
}
