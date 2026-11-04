// Window-level shortcuts. Two rules hold this together:
//
//  - a keystroke aimed at a text field is left alone, except Ctrl+P;
//  - Escape unwinds exactly one thing per press, never two.
//
// "Ctrl" is the shortcut modifier: Command on a Mac (logic/keys.ts), where a
// plain-letter shortcut also stands aside while Command or Control is held,
// so ⌘F or ⌃T is never mistaken for F or T.

import { useEffect } from "react";
import { backend, capabilities } from "../backend";
import { playPrevious, recordFromPlayer, runAction } from "../actions";
import { focusSearch, typingInto } from "../focus";
import { exitFullscreen, toggleFullscreen } from "../fullscreen";
import { fullscreenNow, refreshFullscreen } from "./useFullscreen";
import { blocksPlainKey, shortcutModifier } from "../logic/keys";
import { ALL_CATEGORIES, railItemKind } from "../logic/railItems";
import { playingKind } from "../logic/movies";
import { playerRecord } from "../logic/recordings";
import { VOD_SKIP_SECONDS } from "../logic/vodTransport";
import { IS_MAC } from "../platform";
import { run } from "../store";
import type { State } from "../store";
import type { Action } from "../store";
import type { Dispatch } from "react";

export function useGlobalKeys(state: State, dispatch: Dispatch<Action>) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (shortcutModifier(e, IS_MAC) && (e.key === "p" || e.key === "P")) {
        e.preventDefault();
        dispatch({ type: "palette", open: !state.paletteOpen });
        return;
      }

      // Quit, from anywhere, the way desktop apps do. Closing the window is
      // quitting: there is one window and nothing stays behind it (the
      // recording service is its own process and keeps its schedule).
      if (shortcutModifier(e, IS_MAC) && !e.altKey && (e.key === "q" || e.key === "Q")) {
        e.preventDefault();
        runAction("quit", { dispatch, player: state.player, previous: state.previousChannel });
        return;
      }

      if (e.key === "Escape") {
        // One press, one dismissal, in the order things were stacked up. The
        // toolbar's menu is the most recent thing anyone opened whenever it is
        // open — it cannot be opened from behind a modal — so it unwinds first.
        if (state.programmeMenu) {
          // The guide's menu over a programme: a context menu, opened last.
          dispatch({ type: "programmeMenu", menu: null });
        } else if (state.favouriteMenu) {
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
        } else if (state.videoMenu) {
          // The picture's menu: opened last, like the others. A submenu
          // inside it that has focus closes itself first (see SubMenu).
          dispatch({ type: "videoMenu", at: null });
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
      if (blocksPlainKey(e, IS_MAC)) return;

      if (e.key === "/") {
        e.preventDefault();
        // The search box only exists in browse mode, so get there first and
        // focus on the next frame, once it is mounted.
        if (state.mode !== "browse") dispatch({ type: "mode", mode: "browse" });
        // Recordings has no channel search: the list is hidden behind it, so
        // go to the channels first.
        if (railItemKind(state.categoryId) === "recordings") {
          dispatch({ type: "selectCategory", id: ALL_CATEGORIES });
        }
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
      if ((e.key === "s" || e.key === "S") && capabilities.mpv) {
        e.preventDefault();
        dispatch({ type: "stats", open: !state.statsOpen });
        return;
      }

      // Recall, from anywhere: a remote's "last" button. Not a player
      // control — after a stop, going back to what was on still makes sense.
      // Ctrl+P never reaches here (the palette, above); Alt+P is the system's.
      if ((e.key === "p" || e.key === "P") && !e.altKey && !e.metaKey && capabilities.playback) {
        e.preventDefault();
        playPrevious(dispatch, state.previousChannel);
        return;
      }

      // The rest only mean anything with a stream open.
      if (state.player.state === "stopped") return;
      const movie = playingKind(state.player) === "movie";

      // Theater: the picture filling the window, without going fullscreen
      // (that is F). YouTube's key for the same idea, so the hand already
      // knows it. From the list or the guide it opens the player; from the
      // player it goes back to where the docked picture lives.
      if (e.key === "t" || e.key === "T") {
        e.preventDefault();
        dispatch({ type: "mode", mode: state.mode === "watch" ? "browse" : "watch" });
        return;
      }

      // Record what is on: the programme the guide says is on now, on the
      // channel playing. Already recorded, it says so rather than stopping
      // it (the menus' Stop recording does that).
      if ((e.key === "r" || e.key === "R") && !e.altKey && !movie) {
        e.preventDefault();
        void recordFromPlayer(
          dispatch,
          playerRecord(state.recordings, state.nowChannel, state.nowProgramme, Math.floor(Date.now() / 1000)),
        );
        return;
      }

      const skip = movie ? VOD_SKIP_SECONDS : 5;
      switch (e.key) {
        case " ":
          e.preventDefault();
          void run(dispatch, "pause", backend.togglePause());
          break;
        case "ArrowLeft":
          e.preventDefault();
          void run(dispatch, "seek", backend.seekRelative(-skip));
          break;
        case "ArrowRight":
          e.preventDefault();
          void run(dispatch, "seek", backend.seekRelative(skip));
          break;
        case "l":
        case "L":
          // A movie has no live edge.
          if (movie) break;
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
    state.categoryId,
    state.paletteOpen,
    state.player.state,
    state.player.onDemand?.kind,
    state.player.streamId,
    state.query,
    state.settings.guideHours,
    state.settingsOpen,
    state.menuOpen,
    state.aboutOpen,
    state.updateOpen,
    state.favouriteMenu,
    state.programmeMenu,
    state.categoryMenu,
    state.categoryOptions,
    state.videoMenu,
    state.statsOpen,
    state.previousChannel,
    state.recordings,
    state.nowChannel,
    state.nowProgramme,
  ]);
}
