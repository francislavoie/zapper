// The shell. Three things live at this level: the two subscriptions the
// backend pushes into (sync + player), the browse/watch layout, and the
// transparent hole that tells mpv where to draw.
//
// Nothing here may paint an opaque pixel over the video region. The page
// background stays transparent; only chrome surfaces get colour.

import { useEffect, useRef } from "react";
import { backend, isMock, subscribePlayer, subscribeSync } from "./backend";
import { errorMessage } from "./logic/async";
import { useGlobalKeys } from "./hooks/useGlobalKeys";
import { useSettings } from "./hooks/useSettings";
import { useVideoRect } from "./hooks/useVideoRect";
import { useZoom } from "./hooks/useZoom";
import { useApp, useDispatch } from "./store";
import { useCanStack } from "./hooks/useCanStack";
import { saveStacked } from "./stacked";
import { AboutModal } from "./components/AboutModal";
import { Rail } from "./components/Rail";
import { ChannelList } from "./components/ChannelList";
import { CommandPalette } from "./components/CommandPalette";
import { GuideGrid } from "./components/GuideGrid";
import { PlayerChrome } from "./components/PlayerChrome";
import { SettingsModal } from "./components/SettingsModal";
import { Toolbar } from "./components/Toolbar";
import { ZoomBadge } from "./components/ZoomBadge";
import { WatchBand } from "./components/WatchBand";
import { Toasts } from "./components/Toasts";

export function App() {
  const state = useApp();
  const dispatch = useDispatch();
  const dockRef = useRef<HTMLDivElement | null>(null);
  const stageRef = useRef<HTMLDivElement | null>(null);
  const planeRef = useRef<HTMLDivElement | null>(null);
  const browsing = state.mode === "browse";

  // The stacked layout is a preference, not a mode: it applies while watching,
  // and only when the window has the room. Too small a window falls back to
  // the ordinary player and returns here when it grows, so the preference is
  // never silently thrown away.
  const room = useCanStack(state.zoom);
  const stacked = state.mode === "watch" && state.stacked && room;

  useGlobalKeys(state, dispatch);

  // Remembered per machine, like zoom, and written here rather than in the
  // reducer so the reducer stays pure.
  useEffect(() => {
    saveStacked(state.stacked);
  }, [state.stacked]);

  // Ctrl +/-/0 and Ctrl+wheel. Applied before paint, which matters below: the
  // video rect is measured from boxes this has already resized.
  useZoom(state.zoom, dispatch);

  // Read once at boot rather than when the modal opens: the guide needs
  // guideHours whether or not anyone has been to the settings pane.
  useSettings(true);

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
    docked: browsing || stacked,
    // Zoom moves every box on screen, and the video is not a box — mpv draws
    // it beneath the webview from coordinates this app measures and sends. So
    // a zoom change has to re-measure and re-send, or the picture stays at the
    // old layout's position and detaches from its slot.
    zoom: state.zoom,
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
      .then((p) => live && dispatch({ type: "player", value: p }))
      .catch((e) => {
        if (import.meta.env.DEV) console.warn("player_state:", errorMessage(e));
      });

    return () => {
      live = false;
      for (const u of unsubs) u();
    };
  }, [dispatch]);

  return (
    <div className="app" data-mode={state.mode}>
      {/* Mock only. In Tauri this never renders, because a painted rectangle
          here would sit on top of the video plane and hide it. */}
      {isMock ? <div className="mock-video" ref={planeRef} /> : null}

      {/* The window's own title bar. It owns the top of the app, and the stage
          below it owns everything else — including where the video goes. */}
      <Toolbar />

      <div className="stage" ref={stageRef}>
        {browsing ? (
          <div className="browse">
            {/* First in the DOM as well as on screen: the band owns the top of
                the stage, and the list below it scrolls under a picture that
                never moves. */}
            <WatchBand videoRef={dockRef} />
            <Rail />
            <ChannelList />
          </div>
        ) : null}

        {state.mode === "guide" ? <GuideGrid /> : null}

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
      </div>

      {state.paletteOpen ? <CommandPalette /> : null}
      {state.settingsOpen ? <SettingsModal /> : null}
      {state.aboutOpen ? <AboutModal /> : null}

      <ZoomBadge />
      <Toasts />
    </div>
  );
}
