// Window-level shortcuts. Two rules hold this together:
//
//  - a keystroke aimed at a text field is left alone, except Ctrl+P;
//  - Escape unwinds exactly one thing per press, never two.

import { useEffect } from "react";
import { backend } from "../backend";
import { focusSearch, typingInto } from "../focus";
import { toggleFullscreen } from "../fullscreen";
import { clampHours } from "../logic/guide";
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
        if (state.menuOpen) {
          dispatch({ type: "menu", open: false });
        } else if (state.aboutOpen) {
          dispatch({ type: "about", open: false });
        } else if (state.settingsOpen) {
          dispatch({ type: "settings", open: false });
        } else if (state.paletteOpen) {
          dispatch({ type: "palette", open: false });
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
        const page = clampHours(state.settings.guideHours);
        if (e.key === "ArrowLeft") {
          e.preventDefault();
          dispatch({ type: "guideShift", hours: -page });
          return;
        }
        if (e.key === "ArrowRight") {
          e.preventDefault();
          dispatch({ type: "guideShift", hours: page });
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
        void toggleFullscreen().catch((err: unknown) =>
          dispatch({ type: "toast", text: `fullscreen: ${String(err)}` }),
        );
        return;
      }

      // The rest only mean anything with a stream open.
      if (state.player.state === "stopped") return;

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
  ]);
}
