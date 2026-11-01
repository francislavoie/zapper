// Window-level shortcuts. Two rules hold this together:
//
//  - a keystroke aimed at a text field is left alone, except Ctrl+P;
//  - Escape unwinds exactly one thing per press, never two.

import { useEffect } from "react";
import { backend } from "../backend";
import { focusSearch, typingInto } from "../focus";
import { exitFullscreen, toggleFullscreen } from "../fullscreen";
import { fullscreenNow, refreshFullscreen } from "./useFullscreen";
import { run } from "../store";
import type { State } from "../store";
import type { Action } from "../store";
import type { Dispatch } from "react";

export function useGlobalKeys(state: State, dispatch: Dispatch<Action>) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && (e.key === "p" || e.key === "P")) {
        e.preventDefault();
        dispatch({ type: "palette", open: !state.paletteOpen });
        return;
      }

      if (e.key === "Escape") {
        // One press, one dismissal, in the order things were stacked up. The
        // toolbar's menu is the most recent thing anyone opened whenever it is
        // open — it cannot be opened from behind a modal — so it unwinds first.
        if (state.favouriteMenu) {
          // A context menu is always the most recent thing on screen: it
          // cannot be opened from behind anything.
          dispatch({ type: "favouriteMenu", menu: null });
        } else if (state.categoryMenu) {
          // The same, for the category rail's. Only one of the two can be
          // open: each closes on any right-click outside itself.
          dispatch({ type: "categoryMenu", menu: null });
        } else if (state.categoryOptions) {
          // The rail's options menu: also a menu, also opened last.
          dispatch({ type: "categoryOptions", menu: null });
        } else if (state.menuOpen) {
          dispatch({ type: "menu", open: false });
        } else if (state.aboutOpen) {
          dispatch({ type: "about", open: false });
        } else if (state.updateOpen) {
          // Above settings: the settings pane's "Check now" opens it there.
          dispatch({ type: "updateDialog", open: false });
        } else if (state.settingsOpen) {
          dispatch({ type: "settings", open: false });
        } else if (state.paletteOpen) {
          dispatch({ type: "palette", open: false });
        } else if (fullscreenNow()) {
          // Fullscreen is the one thing Escape means everywhere else on the
          // desktop, so it is the whole meaning here: leave fullscreen and
          // nothing more. Falling through to the mode steps below made the
          // first press swap the picture for the browse view while staying
          // fullscreen. Overlays above still close first — they are on top.
          void exitFullscreen()
            .then(refreshFullscreen)
            .catch((err: unknown) =>
              dispatch({ type: "toast", text: `fullscreen: ${String(err)}` }),
            );
        } else if (state.query !== "") {
          dispatch({ type: "query", text: "" });
        } else if (state.mode === "guide") {
          dispatch({ type: "mode", mode: "browse" });
        } else if (state.mode === "watch") {
          dispatch({ type: "mode", mode: "browse" });
        } else if (state.player.state !== "stopped") {
          dispatch({ type: "mode", mode: "watch" });
        }
        e.preventDefault();
        return;
      }

      if (typingInto(e.target)) return;

      if (e.key === "/") {
        e.preventDefault();
        // The search box only exists in browse mode, so get there first and
        // focus on the next frame, once it is mounted.
        if (state.mode !== "browse") dispatch({ type: "mode", mode: "browse" });
        requestAnimationFrame(focusSearch);
        return;
      }

      // The guide is a view, not a player function: it works with nothing
      // playing, and it owns the arrow keys while it is up.
      if (e.key === "g" || e.key === "G") {
        e.preventDefault();
        dispatch({ type: "mode", mode: state.mode === "guide" ? "browse" : "guide" });
        return;
      }

      if (state.mode === "guide") {
        // An hour per press, like the ‹ › buttons: a whole screen was a jump
        // big enough to lose your place in.
        if (e.key === "ArrowLeft") {
          e.preventDefault();
          dispatch({ type: "guideShift", hours: -1 });
          return;
        }
        if (e.key === "ArrowRight") {
          e.preventDefault();
          dispatch({ type: "guideShift", hours: 1 });
          return;
        }
        if (e.key === "Home") {
          e.preventDefault();
          dispatch({ type: "guideNow" });
          return;
        }
      }

      // Fullscreen is a window property whether or not anything is playing.
      if (e.key === "f" || e.key === "F") {
        e.preventDefault();
        void toggleFullscreen()
          .then(refreshFullscreen)
          .catch((err: unknown) =>
            dispatch({ type: "toast", text: `fullscreen: ${String(err)}` }),
          );
        return;
      }

      // Stats for nerds, from anywhere: the panel says what it can with
      // nothing playing, and its graphs are already recording when something is.
      if (e.key === "s" || e.key === "S") {
        e.preventDefault();
        dispatch({ type: "stats", open: !state.statsOpen });
        return;
      }

      // The rest only mean anything with a stream open.
      if (state.player.state === "stopped") return;

      // Theater: the picture filling the window, without going fullscreen
      // (that is F). YouTube's key for the same idea, so the hand already
      // knows it. From the list or the guide it opens the player; from the
      // player it goes back to where the docked picture lives.
      if (e.key === "t" || e.key === "T") {
        e.preventDefault();
        dispatch({ type: "mode", mode: state.mode === "watch" ? "browse" : "watch" });
        return;
      }

      switch (e.key) {
        case " ":
          e.preventDefault();
          void run(dispatch, "pause", backend.togglePause());
          break;
        case "ArrowLeft":
          e.preventDefault();
          void run(dispatch, "seek", backend.seekRelative(-5));
          break;
        case "ArrowRight":
          e.preventDefault();
          void run(dispatch, "seek", backend.seekRelative(5));
          break;
        case "l":
        case "L":
          e.preventDefault();
          void run(dispatch, "seek to live", backend.seekLive());
          break;
      }
    };

    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    dispatch,
    state.mode,
    state.paletteOpen,
    state.player.state,
    state.query,
    state.settings.guideHours,
    state.settingsOpen,
    state.menuOpen,
    state.aboutOpen,
    state.updateOpen,
    state.favouriteMenu,
    state.categoryMenu,
    state.categoryOptions,
    state.statsOpen,
  ]);
}
