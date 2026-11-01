// The shell. Three things live at this level: the two subscriptions the
// backend pushes into (sync + player), the browse/watch layout, and the
// transparent hole that tells mpv where to draw.
//
// Nothing here may paint an opaque pixel over the video region. The page
// background stays transparent; only chrome surfaces get colour.

import { Activity, useEffect, useLayoutEffect, useRef } from "react";
import { backend, isMock, subscribePlayer, subscribeSync } from "./backend";
import { paintMockStill } from "./mock";
import { loadCategories } from "./actions";
import { errorMessage } from "./logic/async";
import { useGlobalKeys } from "./hooks/useGlobalKeys";
import { useWheelSteps } from "./hooks/useWheelSteps";
import { useSettings } from "./hooks/useSettings";
import { useVideoRect } from "./hooks/useVideoRect";
import { useZoom } from "./hooks/useZoom";
import { useApp, useDispatch } from "./store";
import { useCanStack } from "./hooks/useCanStack";
import { useFullscreen } from "./hooks/useFullscreen";
import { loadSession, saveSession, watchForExit } from "./session";
import { applyVideoWidth, saveVideoWidth } from "./videoSize";
import { fittedVideoWidth } from "./logic/videoSize";
import { useVideoRoom } from "./hooks/useVideoRoom";
import { applyRailWidth, saveRailWidth } from "./railWidth";
import { saveStacked } from "./stacked";
import { saveCategoryHidden, saveCategoryPins, saveCategorySort } from "./categoryPrefs";
import { AboutModal } from "./components/AboutModal";
import { Rail } from "./components/Rail";
import { ChannelList } from "./components/ChannelList";
import { CommandPalette } from "./components/CommandPalette";
import { GuideGrid } from "./components/GuideGrid";
import { PlayerChrome } from "./components/PlayerChrome";
import { SettingsModal } from "./components/SettingsModal";
import { UpdateModal } from "./components/UpdateModal";
import { useUpdates } from "./hooks/useUpdates";
import { Toolbar } from "./components/Toolbar";
import { ResizeEdges } from "./components/ResizeEdges";
import { ZoomBadge } from "./components/ZoomBadge";
import { WatchBand } from "./components/WatchBand";
import { Toasts } from "./components/Toasts";
import { Boundary } from "./components/Crashed";
import { StatsPanel } from "./components/StatsPanel";
import { VideoMenu } from "./components/VideoMenu";
import { TooltipHost } from "./components/TooltipHost";

export function App() {
  const state = useApp();
  const dispatch = useDispatch();
  const dockRef = useRef<HTMLDivElement | null>(null);
  const stageRef = useRef<HTMLDivElement | null>(null);
  const planeRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (import.meta.env.DEV && isMock && planeRef.current) paintMockStill(planeRef.current);
  }, []);
  const browsing = state.mode === "browse";
  const guiding = state.mode === "guide";

  // The stacked layout is a preference, not a mode: it applies while watching,
  // and only when the window has the room. Too small a window falls back to
  // the ordinary player and returns here when it grows, so the preference is
  // never silently thrown away.
  const room = useCanStack(state.zoom);
  const stacked = state.mode === "watch" && state.stacked && room;

  // Fullscreen means the picture, not the app with a bigger window around it.
  // The toolbar is the app's own title bar and there is no title bar in a
  // fullscreen video, so it goes, and the stage takes the whole window. The
  // video rect is measured from the stage, so that is all it takes to make
  // the picture fill the screen.
  const fullscreen = useFullscreen();

  // Leaving fullscreen goes back where it was entered from.
  //
  // Only when it was entered from somewhere else: going fullscreen from the
  // docked picture switches to the player on the way, because that is what
  // makes the picture fill the screen, and dropping the viewer there on the
  // way out would be a one-way door. Fullscreen entered from the player
  // itself leaves them in the player, which is where they already were.
  //
  // Keyed on the transition rather than on the value: fullscreen is false
  // for a moment before the window reports otherwise, and acting on that
  // would undo the change as it was being made.
  const wasFullscreen = useRef(false);
  const cameFrom = state.fullscreenFrom;
  useEffect(() => {
    if (wasFullscreen.current && !fullscreen && cameFrom) {
      dispatch({ type: "mode", mode: cameFrom });
      dispatch({ type: "fullscreenFrom", mode: null });
    }
    wasFullscreen.current = fullscreen;
  }, [fullscreen, cameFrom, dispatch]);

  useGlobalKeys(state, dispatch);
  // Wheel scrolling that steps rather than glides; see logic/wheel.ts.
  useWheelSteps();

  // The updater's state, for the title bar's badge and the dialog.
  useUpdates();

  // Remembered per machine, like zoom, and written here rather than in the
  // reducer so the reducer stays pure.
  useEffect(() => {
    saveStacked(state.stacked);
  }, [state.stacked]);

  // Where you were. Same reasoning and the same place as the two above: not a
  // setting, not synced, and a property of this machine rather than of the
  // account.
  useEffect(() => {
    saveSession({
      categoryId: state.categoryId,
      railTab: state.railTab,
      railCollapsed: state.railCollapsed,
    });
  }, [state.categoryId, state.railTab, state.railCollapsed]);

  // Closing mid-scroll must still remember where you were; writes are
  // otherwise coalesced and the last one would be lost.
  useEffect(watchForExit, []);

  // The channel from last time, restored as metadata and nothing else.
  //
  // Deliberately not played. Many providers allow a single connection, and
  // opening a live stream at startup would spend it — and the bandwidth —
  // on a guess about what this launch is for. Browsing is at least as likely
  // as watching. So the band comes back showing the channel, its programme
  // and its picture slot, and one click starts it.
  //
  // Runs once. A later channel change is not a restore, and re-running this
  // would fight the player for `nowChannel`.
  const restored = useRef(false);
  useEffect(() => {
    if (restored.current) return;
    restored.current = true;
    const id = loadSession().lastStreamId;
    if (id === null) return;
    void backend
      .channelsByIds([id])
      .then((rows) => {
        // The reducer decides whether this is still wanted: the user may
        // have started something while the lookup was in flight.
        const channel = rows[0];
        if (channel) dispatch({ type: "ready", channel });
      })
      .catch(() => {
        // A channel that has gone, or a mirror not yet built. Starting on
        // "Nothing playing" is the correct fallback and needs no apology.
      });
  }, [dispatch]);

  // What to have ready next time.
  useEffect(() => {
    if (state.nowChannel) saveSession({ lastStreamId: state.nowChannel.streamId });
  }, [state.nowChannel]);

  /** Whether the remembered compressor setting has been dealt with.
   *
   *  This gate is the whole difficulty. The player starts out reporting the
   *  compressor off, because nothing has asked the backend yet, and a plain
   *  "save whatever it currently is" effect would fire on mount and write
   *  that `false` over the remembered `true` before anything had a chance to
   *  restore it. So nothing is saved until the restore has happened. */
  const audioRestored = useRef(false);

  useEffect(() => {
    if (!audioRestored.current) return;
    saveSession({ compressor: state.player.compressor });
  }, [state.player.compressor]);

  // Volume and mute, behind the same gate and for the same reason: the
  // player reports mpv's default until it has been told otherwise, and saving
  // that on mount would forget the level the app was closed at.
  useEffect(() => {
    if (!audioRestored.current) return;
    saveSession({ volume: state.player.volume, muted: state.player.muted });
  }, [state.player.volume, state.player.muted]);

  // Ctrl +/-/0 and Ctrl+wheel. Applied before paint, which matters below: the
  // video rect is measured from boxes this has already resized.
  useZoom(state.zoom, dispatch);

  // Before paint for the same reason: this writes the custom property the
  // slot is sized from, and the rect is measured from the slot afterwards.
  // What is drawn is the remembered width capped by the room the stage has;
  // what is saved, below, is the remembered width alone.
  useVideoRoom(stageRef, state.zoom, dispatch);
  const videoWidthShown = fittedVideoWidth(state.videoWidthRem, state.videoRoomRem);
  useLayoutEffect(() => {
    applyVideoWidth(videoWidthShown);
  }, [videoWidthShown]);

  useLayoutEffect(() => {
    applyRailWidth(state.railWidthRem);
  }, [state.railWidthRem]);

  useEffect(() => {
    saveVideoWidth(state.videoWidthRem);
  }, [state.videoWidthRem]);

  useEffect(() => {
    saveRailWidth(state.railWidthRem);
  }, [state.railWidthRem]);

  // Here rather than in the rail: the rail sits inside an <Activity> that
  // unmounts its effects while the Favourites tab is showing.
  useEffect(() => {
    saveCategoryPins(state.categoryPins);
  }, [state.categoryPins]);

  useEffect(() => {
    saveCategorySort(state.categorySort);
  }, [state.categorySort]);

  useEffect(() => {
    saveCategoryHidden(state.categoryHidden);
  }, [state.categoryHidden]);

  // Read once at boot rather than when the modal opens: the guide needs
  // guideHours whether or not anyone has been to the settings pane.
  useSettings(true);

  // The catalogue, loaded by the app rather than by the rail that shows it.
  // The favourites name each channel's category from the same list, and a
  // hidden <Activity> subtree does not run effects — so while the
  // Favourites tab was the one on screen, nothing fetched the categories it
  // needed and every favourite lost its category line.
  const categoriesStatus = state.categories.status;
  useEffect(() => {
    if (categoriesStatus === "idle") void loadCategories(dispatch);
  }, [categoriesStatus, dispatch]);

  // Only browse has a slot to dock into. The guide covers the stage with an
  // opaque grid, so the video goes back to filling the stage underneath it —
  // mpv draws below the webview, and covered is covered.
  //
  // Undocked is the stage, not the window: the toolbar is opaque chrome across
  // the top, and video drawn behind it would simply be missing. The stage is
  // measured rather than the toolbar's height subtracted by hand, so the bar
  // can change height without a second place needing to hear about it.
  useVideoRect({
    dockRef,
    stageRef,
    // Stacked docks too: the picture goes in a slot across the top, and the
    // rect is measured from that slot exactly as it is from the browse dock.
    // No special case reaches mpv — this is a layout change, nothing more.
    // The guide docks as well. Undocked, the video would fill the stage
    // *underneath* an opaque grid — drawn, paid for, and invisible — so the
    // guide leaves the same hole the browse band does.
    docked: browsing || stacked || state.mode === "guide",
    // Zoom moves every box on screen, and the video is not a box — mpv draws
    // it beneath the webview from coordinates this app measures and sends. So
    // a zoom change has to re-measure and re-send, or the picture stays at the
    // old layout's position and detaches from its slot.
    zoom: state.zoom,
    // Resizing the picture moves the slot it is measured from. Listed for
    // the same reason as `zoom`: it is not used in the arithmetic, but every
    // box it measures moves when it changes.
    videoWidthRem: videoWidthShown,
    // A fullscreen change resizes the stage, and that change is animated —
    // in both directions. Without it the way in animated (the mode often
    // changed too) and the way out snapped, which reads as a glitch rather
    // than as a different animation.
    fullscreen,
    paintRef: isMock ? planeRef : undefined,
    onError: (m) => dispatch({ type: "toast", text: `video placement: ${m}` }),
  });

  // Both subscriptions, plus one read of current state so a reload does not
  // sit blank waiting for the next push.
  useEffect(() => {
    let live = true;
    const unsubs: Array<() => void> = [];

    void subscribeSync((s) => dispatch({ type: "sync", value: s }))
      .then((u) => (live ? unsubs.push(u) : u()))
      .catch(() => {});
    void subscribePlayer((p) => dispatch({ type: "player", value: p }))
      .then((u) => (live ? unsubs.push(u) : u()))
      .catch(() => {});

    backend
      .syncStatus()
      .then((s) => live && dispatch({ type: "sync", value: s }))
      .catch(() => {
        // Not fatal and not worth a toast on boot: the strip simply stays
        // hidden until the backend has something to say.
      });
    backend
      .playerState()
      .then((p) => {
        if (!live) return;
        dispatch({ type: "player", value: p });
        // Applied here rather than on a timer because this is the first
        // moment the backend has answered, which is the first moment it is
        // known to be able to take the call. mpv's `af` is a global
        // property, not a per-file one, so setting it once now keeps it on
        // across every channel change afterwards.
        audioRestored.current = true;
        const saved = loadSession();
        if (saved.compressor && !p.compressor) {
          void backend.setCompressor(true).catch(() => {
            // The preference is a nicety; failing to restore it is not worth
            // a toast on boot.
          });
        }
        // The level the app was closed at. Like the compressor, mpv's
        // volume is global rather than per file, so once is enough.
        if (saved.volume !== null && Math.abs(saved.volume - p.volume) > 0.5) {
          void backend.setVolume(saved.volume).catch(() => {});
        }
        if (saved.muted !== p.muted) {
          void backend.setMuted(saved.muted).catch(() => {});
        }
      })
      .catch((e) => {
        // Still open the gate: the preference cannot be restored, but the
        // user's later toggles must still be remembered.
        audioRestored.current = true;
        if (import.meta.env.DEV) console.warn("player_state:", errorMessage(e));
      });

    return () => {
      live = false;
      for (const u of unsubs) u();
    };
  }, [dispatch]);

  return (
    <div className={`app ${fullscreen ? "fullscreen" : ""}`} data-mode={state.mode}>
      {/* Mock only. In Tauri this never renders, because a painted rectangle
          here would sit on top of the video plane and hide it. */}
      {isMock ? <div className="mock-video" ref={planeRef} /> : null}

      {/* The window's own title bar. It owns the top of the app, and the stage
          below it owns everything else — including where the video goes. */}
      {/* Outside every boundary on purpose. This is the window's title bar
          as well as the app's: if it stops rendering, so does the way to
          close, move or resize the window. It has to survive whatever the
          content does. */}
      {fullscreen ? null : <Toolbar />}
      <ResizeEdges fullscreen={fullscreen} />

      <div className="stage" ref={stageRef}>
        {/* Everything below the toolbar. If a pane throws, the window is
            still a window: the bar above this renders regardless, so it can
            still be moved, resized and closed. */}
        <Boundary what="This view">
        {/* Browse and the guide share one band, mounted once, and each view
            below it is kept alive while the other is shown. As two separate
            trees, each with its own band, switching views would throw the
            band away and fetch now/next again, and coming back to the list
            would re-fetch its pages and lose the scroll — every toggle a
            visible reload. A hidden <Activity> keeps state and DOM and only
            pauses effects. The whole thing is hidden, not unmounted, while
            watching full size, so the round trip through the player costs
            nothing either.

            The rail is shared the same way. Both views are channels by
            category, so both show the one rail beside them — one instance,
            so its tab, filter and scroll are the same whichever view is up,
            and choosing a category in it filters whichever list is showing.
            Folded away, it is hidden rather than unmounted for the same
            reasons, and the grid drops its column (.rail-collapsed) so the
            view takes the whole width. */}
        <Activity mode={browsing || guiding ? "visible" : "hidden"}>
          <div
            className={`browse ${state.railCollapsed ? "rail-collapsed" : ""}`}
            data-view={guiding ? "guide" : "list"}
          >
            {/* First in the DOM as well as on screen: the band owns the top of
                the stage, and what is below it scrolls under a picture that
                never moves. mpv draws beneath the webview, so in both views
                the band is where the hole for the picture is. */}
            <WatchBand videoRef={dockRef} />
            <Activity mode={state.railCollapsed ? "hidden" : "visible"}>
              <Rail />
            </Activity>
            <Activity mode={guiding ? "hidden" : "visible"}>
              <ChannelList />
            </Activity>
            <Activity mode={guiding ? "visible" : "hidden"}>
              <GuideGrid besideRail />
            </Activity>
          </div>
        </Activity>

        {/* Over the picture in every mode, which is where a right-click on
            the stream is answered and where the diagnostics belong. */}
        {state.videoMenu ? (
          <VideoMenu
            at={state.videoMenu}
            onClose={() => dispatch({ type: "videoMenu", at: null })}
          />
        ) : null}
        {state.statsOpen ? (
          <StatsPanel onClose={() => dispatch({ type: "stats", open: false })} />
        ) : null}

        {/* Chrome belongs to watch mode. Browsing shows the docked picture and
            the metadata beside it, not a full set of transport controls. */}
        {state.mode === "watch" && !stacked ? <PlayerChrome /> : null}

        {/* Stacked: the picture across the top at full width, the guide
            underneath it. The slot is an empty transparent box — the video is
            not in the DOM, it is drawn beneath the webview where this box
            says — and the controls sit over it, as they do in the ordinary
            player. */}
        {stacked ? (
          <div className="stacked">
            <div className="stacked-video" ref={dockRef}>
              <PlayerChrome />
            </div>
            <div className="stacked-guide">
              <GuideGrid />
            </div>
          </div>
        ) : null}
        </Boundary>
      </div>

      {/* Each of these is its own boundary: a modal that throws must not
          take the view behind it with it. */}
      <Boundary what="The command palette">
        {state.paletteOpen ? <CommandPalette /> : null}
      </Boundary>
      <Boundary what="Settings">
        {state.settingsOpen ? <SettingsModal /> : null}
      </Boundary>
      <Boundary what="About">{state.aboutOpen ? <AboutModal /> : null}</Boundary>
      <Boundary what="Updates">{state.updateOpen ? <UpdateModal /> : null}</Boundary>

      <ZoomBadge />
      <Toasts />
      {/* Once, for every tip in the app: targets carry `data-tip` and this
          finds them. Outside every boundary for the same reason as the
          toolbar — it is part of how the app is used, not of one view. */}
      <TooltipHost />
    </div>
  );
}
