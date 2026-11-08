// Window-level shortcuts. Two rules hold this together:
//
//  - a keystroke aimed at a text field is left alone, except Ctrl+P;
//  - Escape unwinds exactly one thing per press, never two.
//
// The mouse's back button walks the same list (logic/escape.ts), and with
// nothing left open goes back through the tabs and rail choices visited, as
// its forward button goes forward again (logic/navHistory.ts).
//
// "Ctrl" is the shortcut modifier: Command on a Mac (logic/keys.ts), where a
// plain-letter shortcut also stands aside while Command or Control is held,
// so ⌘F or ⌃T is never mistaken for F or T.

import { useEffect, useRef } from "react";
import { backend, backendKind, capabilities } from "../backend";
import { escapeStep, type EscapeContext } from "../logic/escape";
import {
  MOUSE_BACK,
  MOUSE_FORWARD,
  SAME_PRESS_MS,
  buttonLatch,
  isSideButton,
  goBack,
  goForward,
  startHistory,
  visit,
  type NavHistory,
  type Place,
} from "../logic/navHistory";
import {
  goToContinue,
  playNextEpisode,
  playPrevious,
  recordFromPlayer,
  resumeHeld,
  runAction,
  seekBy,
} from "../actions";
import { focusSearch, typingInto } from "../focus";
import { exitFullscreen, toggleFullscreen } from "../fullscreen";
import { fullscreenNow, refreshFullscreen } from "./useFullscreen";
import { useSubtitles } from "./useSubtitles";
import { blocksPlainKey, shortcutModifier } from "../logic/keys";
import { moviesAvailable, playingKind } from "../logic/movies";
import { showsAvailable } from "../logic/shows";
import { ALL_CATEGORIES, railItemKind } from "../logic/railItems";
import { continueShown } from "../logic/continueWatching";
import { playerRecord } from "../logic/recordings";
import { upNextTakesEnter, type UpNext } from "../logic/upNext";
import { VOD_SKIP_SECONDS, isOnDemandKind } from "../logic/vodTransport";
import { IS_MAC } from "../platform";
import { closeFindSubtitles, useFindSubtitles } from "../onlineSubtitles";
import { run } from "../store";
import type { State } from "../store";
import type { Action } from "../store";
import type { Dispatch } from "react";

/** Every event a side button's press can arrive as. All are cancelled; the
 *  latch picks the one that acts. */
const BUTTON_EVENTS = ["pointerdown", "mousedown", "pointerup", "mouseup", "auxclick"] as const;

/** What the desktop app's Linux webview dispatches on the window for a side
 *  button's release, with `detail` "back" or "forward"
 *  (src-tauri/src/surface/linux/overlay.rs). */
const SIDE_BUTTON_EVENT = "zapper:side-button";

export function useGlobalKeys(
  state: State,
  dispatch: Dispatch<Action>,
  upNext: UpNext,
  /** The player is across the top with a list under it (App's `stacked`). */
  stacked: boolean,
  /** The Resume question is up (App's useResumePrompt). */
  promptShown = false,
) {
  // The episode Up next would play: the same object while the count runs.
  const upNextEpisode = upNext.kind === "none" ? null : upNext.next;
  // Read when C is pressed, rather than listened again for every change of
  // the tracks or the settings it depends on.
  const latestSubtitles = useSubtitles();
  // Its own store (src/onlineSubtitles.ts), not the app's.
  const findOpen = useFindSubtitles() !== null;

  // The tabs and rail choices visited, for the mouse's back and forward
  // buttons once nothing is open (logic/navHistory.ts). Each place the
  // store moves to is a step; a step taken by those buttons arrives where
  // the history already is, and is not one.
  const place: Place = {
    section: state.section,
    rail:
      state.section === "movies"
        ? state.movies.selection
        : state.section === "shows"
          ? state.shows.selection
          : state.categoryId,
  };
  const nav = useRef<NavHistory>(startHistory(place));
  useEffect(() => {
    nav.current = visit(nav.current, { section: place.section, rail: place.rail });
  }, [place.section, place.rail]);
  const latch = useRef(buttonLatch());
  /** When a side button last acted, on performance.now()'s clock. */
  const lastButton = useRef(-Infinity);
  const unwindRef = useRef<(via: "key" | "mouse") => void>(() => {});

  // A browser's own Back — its toolbar button, Alt+Left, or a side button
  // it acts on before the page can cancel it — would leave the app: the
  // page is one history entry. So outside the desktop app a second entry,
  // the sentinel, is kept on top of it: pushed once, at the first press of
  // anything (Chromium's Back skips entries a page adds unprompted, and
  // Escape is not a press that counts), and kept across reloads. When Back
  // leaves it, `history.forward()` returns to that same entry — never a
  // new one, which would be pushed without a press and so skipped — and the
  // app goes back a step instead. The return is itself a pop, onto the
  // sentinel, and is passed over. A side button that has already acted has
  // its pop absorbed rather than acted on twice. The desktop app's webview
  // has no history to go through, and cancelling the button is enough
  // there.
  useEffect(() => {
    if (backendKind === "tauri") return;
    const SENTINEL = "back";
    let pushed = (window.history.state as { zapper?: string } | null)?.zapper === SENTINEL;
    const push = (e: Event) => {
      if (pushed) return;
      if (e instanceof KeyboardEvent && e.key === "Escape") return;
      pushed = true;
      window.history.pushState({ zapper: SENTINEL }, "");
    };
    const onPop = (e: PopStateEvent) => {
      if ((e.state as { zapper?: string } | null)?.zapper === SENTINEL) return;
      window.history.forward();
      if (performance.now() - lastButton.current < SAME_PRESS_MS) return;
      unwindRef.current("mouse");
    };
    window.addEventListener("pointerdown", push, true);
    window.addEventListener("keydown", push, true);
    window.addEventListener("popstate", onPop);
    return () => {
      window.removeEventListener("pointerdown", push, true);
      window.removeEventListener("keydown", push, true);
      window.removeEventListener("popstate", onPop);
    };
  }, []);
  const subtitles = useRef(latestSubtitles);
  subtitles.current = latestSubtitles;
  // What the handlers below read, as of the latest render. They are added
  // once and read it when they run, so no field they use can be missing
  // from a list of dependencies and leave them acting on an old state.
  const latest = useRef({ state, upNextEpisode, stacked, promptShown, findOpen });
  latest.current = { state, upNextEpisode, stacked, promptShown, findOpen };
  useEffect(() => {
    const handlers = () => {
      const { state, upNextEpisode, stacked, promptShown, findOpen } = latest.current;
      // The Up next card in the full player (docked, no key is taken from the
      // lists beside it: its buttons are in the tab order).
      const card = state.mode === "watch" && upNextEpisode ? state.shows.playing : null;
      // Enter only where the card is the player's whole business: not over
      // a list (stacked), where Enter is the list's, nor under anything open.
      const cardInFullPlayer =
        card !== null &&
        !stacked &&
        !state.paletteOpen &&
        !state.settingsOpen &&
        !state.aboutOpen &&
        !state.updateOpen &&
        !state.menuOpen &&
        !state.videoMenu &&
        !findOpen;

      const overlayOpen = () =>
        state.paletteOpen ||
        state.settingsOpen ||
        state.aboutOpen ||
        state.updateOpen ||
        state.menuOpen ||
        state.videoMenu !== null ||
        findOpen;

      // What is open, for the Escape chain (logic/escape.ts).
      const context = (): EscapeContext => ({
        findSubtitles: findOpen,
        programmeMenu: state.programmeMenu !== null,
        favouriteMenu: state.favouriteMenu !== null,
        categoryMenu: state.categoryMenu !== null,
        categoryOptions: state.categoryOptions !== null,
        moviesMenu: state.movies.menu !== null,
        showsMenu: state.shows.menu !== null,
        continueMenu: state.continue.menu !== null,
        videoMenu: state.videoMenu !== null,
        menuOpen: state.menuOpen,
        aboutOpen: state.aboutOpen,
        updateOpen: state.updateOpen,
        settingsOpen: state.settingsOpen,
        paletteOpen: state.paletteOpen,
        // Only while it shows: not behind the player, nor for a tab gone.
        movieSheet:
          state.mode === "browse" &&
          state.section === "movies" &&
          state.movies.sheetId !== null &&
          moviesAvailable(state.movies.summary),
        showSheet:
          state.mode === "browse" &&
          state.section === "shows" &&
          state.shows.sheetId !== null &&
          showsAvailable(state.shows.summary),
        showPane:
          (state.mode === "browse" || stacked) &&
          state.section === "shows" &&
          state.shows.paneOpen &&
          state.shows.selected !== null &&
          showsAvailable(state.shows.summary) &&
          // Continue watching's list opens no pane (ShowsList).
          !continueShown(state.shows.selection, state.shows.query),
        // Only in the full player: docked, Escape is the list's.
        resumePrompt: promptShown && state.mode === "watch" && !stacked,
        upNextCard: card !== null,
        fullscreen: fullscreenNow(),
        // A search belongs to the tab it was typed on.
        query:
          state.section === "movies"
            ? state.movies.query !== ""
            : state.section === "shows"
              ? state.shows.query !== ""
              : state.query !== "",
        // The tab's own search box, not any field (the rail's filter).
        queryFocused:
          typingInto(document.activeElement) &&
          document.activeElement?.closest(".list-search") !== null,
        mode: state.mode,
        playing: state.player.state !== "stopped",
      });

      /** Undoes the top-most thing, as Escape (`key`) or the mouse's back
       *  button (`mouse`) does. */
      const unwind = (via: "key" | "mouse") => {
        const step = escapeStep(context(), via);
        switch (step) {
          case "findSubtitles":
            closeFindSubtitles();
            break;
          case "programmeMenu":
            dispatch({ type: "programmeMenu", menu: null });
            break;
          case "favouriteMenu":
            dispatch({ type: "favouriteMenu", menu: null });
            break;
          case "categoryMenu":
            dispatch({ type: "categoryMenu", menu: null });
            break;
          case "categoryOptions":
            dispatch({ type: "categoryOptions", menu: null });
            break;
          case "moviesMenu":
            dispatch({ type: "movies/menu", menu: null });
            break;
          case "showsMenu":
            dispatch({ type: "shows/menu", menu: null });
            break;
          case "continueMenu":
            dispatch({ type: "continue/menu", menu: null });
            break;
          case "videoMenu":
            // A submenu inside it that has focus closes itself first on
            // Escape (see SubMenu).
            dispatch({ type: "videoMenu", at: null });
            break;
          case "menu":
            dispatch({ type: "menu", open: false });
            break;
          case "about":
            dispatch({ type: "about", open: false });
            break;
          case "update":
            dispatch({ type: "updateDialog", open: false });
            break;
          case "settings":
            dispatch({ type: "settings", open: false });
            break;
          case "palette":
            dispatch({ type: "palette", open: false });
            break;
          case "movieSheet":
            dispatch({ type: "movies/sheet", id: null });
            break;
          case "showSheet":
            dispatch({ type: "shows/sheet", id: null });
            break;
          case "showPane":
            dispatch({ type: "shows/pane", open: false });
            break;
          case "resumePrompt":
            // Closes the question and nothing else: the title stays paused at
            // its place.
            dispatch({ type: "continue/prompt", prompt: null });
            break;
          case "upNextCard":
            // Cancelled, and the player stays as it is. A second press goes
            // on to the steps below.
            if (card) dispatch({ type: "shows/upNextCancel", episodeId: card.episode.episodeId });
            break;
          case "fullscreen":
            // Fullscreen is the one thing Escape means everywhere else on the
            // desktop, so it is the whole meaning here: leave fullscreen and
            // nothing more. Falling through to the mode steps made the first
            // press swap the picture for the browse view while staying
            // fullscreen. Overlays above still close first — they are on top.
            void exitFullscreen()
              .then(refreshFullscreen)
              .catch((err: unknown) =>
                dispatch({ type: "toast", text: `fullscreen: ${String(err)}` }),
              );
            break;
          case "query":
            if (state.section === "movies") dispatch({ type: "movies/query", text: "" });
            else if (state.section === "shows") dispatch({ type: "shows/query", text: "" });
            else dispatch({ type: "query", text: "" });
            break;
          case "browse":
            dispatch({ type: "mode", mode: "browse" });
            break;
          case "watch":
            dispatch({ type: "mode", mode: "watch" });
            break;
          case "history":
            travel(goBack(nav.current, reachable));
            break;
          case "none":
            break;
        }
      };

      /** A place there is still a tab for: Movies and Shows come and go with
       *  the account, and a step back to one that has gone is stepped over. */
      const reachable = (p: Place) =>
        p.section === "live" ||
        (capabilities.onDemand &&
          (p.section === "movies"
            ? moviesAvailable(state.movies.summary)
            : showsAvailable(state.shows.summary)));

      /** Goes to a place from the tab-and-rail history, or nowhere. */
      const travel = (to: ReturnType<typeof goBack>) => {
        if (!to) return;
        nav.current = to.history;
        const { section, rail } = to.place;
        if (section !== state.section) dispatch({ type: "section", section });
        if (section === "movies") {
          if (rail !== state.movies.selection) dispatch({ type: "movies/select", id: rail });
        } else if (section === "shows") {
          if (rail !== state.shows.selection) dispatch({ type: "shows/select", id: rail });
        } else if (rail !== state.categoryId) {
          dispatch({ type: "selectCategory", id: rail });
        }
      };

      // The mouse's back and forward buttons. A press can arrive as more than
      // one event, and which ones differs between engines (see buttonLatch);
      // each is cancelled whatever it is, so neither the webview nor a browser
      // goes through its own history, and one of them acts.
      //
      // Stopped here as well as cancelled: this listens on the window in the
      // capture phase, ahead of the page, so nothing below — a menu's or a
      // dialog's click-outside, a drag — takes the press for its own. The
      // desktop app on Linux sends no DOM events for these buttons at all (see
      // `zapper:side-button` below); on macOS, wry turns them into a synthetic
      // `mousedown`/`mouseup` aimed at `elementFromPoint` with window pixels,
      // which misses above 100 % zoom — the same gap the Linux path closes,
      // still open there.
      const onButton = (e: MouseEvent) => {
        if (!isSideButton(e)) return;
        e.preventDefault();
        e.stopPropagation();
        const down = e.type === "mousedown" || e.type === "pointerdown";
        if (latch.current(down ? "down" : "up", e.button, e.timeStamp)) act(e.button);
      };

      // The desktop app on Linux: the webview's GTK handler takes buttons 8
      // and 9 before wry's (surface/linux/overlay.rs) and says which was
      // released with this event. Through the same latch, so a DOM release
      // of the same press arriving as well cannot act twice.
      const onSideButton = (e: Event) => {
        const detail = (e as CustomEvent<unknown>).detail;
        const button = detail === "back" ? MOUSE_BACK : detail === "forward" ? MOUSE_FORWARD : null;
        if (button !== null && latch.current("up", button, e.timeStamp)) act(button);
      };

      const act = (button: number) => {
        lastButton.current = performance.now();
        if (button === MOUSE_BACK) {
          unwind("mouse");
        } else if (escapeStep(context(), "mouse") === "history") {
          // Forward re-takes the tab and rail steps Back undid. Not a sheet
          // or the player: what Back closed stays closed, as a dismissed
          // dialog does in a browser.
          travel(goForward(nav.current, reachable));
        }
      };

      const onKey = (e: KeyboardEvent) => {
        if (shortcutModifier(e, IS_MAC) && (e.key === "p" || e.key === "P")) {
          e.preventDefault();
          // Not under the subtitle finder: the palette would open beneath it.
          if (!findOpen) dispatch({ type: "palette", open: !state.paletteOpen });
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
          // One press, one dismissal, in the order things were stacked up:
          // logic/escape.ts, which the mouse's back button walks as well.
          unwind("key");
          e.preventDefault();
          return;
        }

        // The subtitle finder keeps its keys; only those with Ctrl (or ⌘)
        // get past it, for quitting and zoom, and none of the shortcuts
        // below may act or open anything beneath it.
        if (findOpen) return;

        if (typingInto(e.target)) return;
        if (blocksPlainKey(e, IS_MAC)) return;

        // Resume, in the full player, by the same rule as Up next's Enter.
        if (
          e.key === "Enter" &&
          promptShown &&
          upNextTakesEnter(e, state.mode === "watch" && !stacked && !overlayOpen())
        ) {
          e.preventDefault();
          void resumeHeld(dispatch);
          return;
        }

        // Up next's Play now, in the full player, when nothing else has taken
        // the key: a focused control (the card's own buttons, Pause after a
        // click) or a list answers its Enter itself.
        if (e.key === "Enter" && card && upNextEpisode && upNextTakesEnter(e, cardInFullPlayer)) {
          e.preventDefault();
          void playNextEpisode(dispatch, card, upNextEpisode);
          return;
        }

        if (e.key === "/") {
          e.preventDefault();
          // The search box only exists in browse mode, so get there first and
          // focus on the next frame, once it is mounted.
          if (state.mode !== "browse") dispatch({ type: "mode", mode: "browse" });
          // Recordings has no channel search: the list is hidden behind it, so
          // go to the channels first. The Movies and Shows tabs have a search
          // of their own, which is the one this reaches while that tab shows.
          if (
            state.section !== "movies" &&
            state.section !== "shows" &&
            railItemKind(state.categoryId) === "recordings"
          ) {
            dispatch({ type: "selectCategory", id: ALL_CATEGORIES });
          }
          requestAnimationFrame(focusSearch);
          return;
        }

        // The Movies tab, where there is one. Not a toggle: Live TV's way back
        // is its tab, and the guide's G.
        if ((e.key === "m" || e.key === "M") && capabilities.onDemand && moviesAvailable(state.movies.summary)) {
          e.preventDefault();
          dispatch({ type: "section", section: "movies" });
          return;
        }

        // The Shows tab, the same way.
        if ((e.key === "n" || e.key === "N") && capabilities.onDemand && showsAvailable(state.shows.summary)) {
          e.preventDefault();
          dispatch({ type: "section", section: "shows" });
          return;
        }

        // Continue watching: on the tab showing if Movies or Shows, else on
        // the one there is (goToContinue). Choosing it plays nothing.
        if (
          (e.key === "w" || e.key === "W") &&
          capabilities.onDemand &&
          (moviesAvailable(state.movies.summary) || showsAvailable(state.shows.summary))
        ) {
          e.preventDefault();
          goToContinue(dispatch, state);
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
        // A movie or an episode: longer skips, and no live edge or programme.
        const movie = isOnDemandKind(playingKind(state.player));

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

        // Subtitles on or off, the transport's toggle: off when they show, on
        // in the language last chosen when they don't, and kept for every
        // channel and title after. Only with subtitles to show.
        if ((e.key === "c" || e.key === "C") && !e.altKey && subtitles.current.available) {
          e.preventDefault();
          subtitles.current.toggle();
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
            void seekBy(dispatch, -skip);
            break;
          case "ArrowRight":
            e.preventDefault();
            void seekBy(dispatch, skip);
            break;
          case "l":
          case "L":
            // A movie or an episode has no live edge.
            if (movie) break;
            e.preventDefault();
            void run(dispatch, "seek to live", backend.seekLive());
            break;
        }
      };

      return { onKey, onButton, onSideButton, unwind };
    };
    const onKey = (e: KeyboardEvent) => handlers().onKey(e);
    const onButton = (e: MouseEvent) => handlers().onButton(e);
    const onSideButton = (e: Event) => handlers().onSideButton(e);
    unwindRef.current = (via) => handlers().unwind(via);

    window.addEventListener("keydown", onKey);
    // Captured, so a component that stops a click's propagation cannot
    // keep the button from the window.
    for (const type of BUTTON_EVENTS) window.addEventListener(type, onButton as EventListener, true);
    window.addEventListener(SIDE_BUTTON_EVENT, onSideButton);
    return () => {
      window.removeEventListener("keydown", onKey);
      for (const type of BUTTON_EVENTS) window.removeEventListener(type, onButton as EventListener, true);
      window.removeEventListener(SIDE_BUTTON_EVENT, onSideButton);
    };
  }, [dispatch]);
}
