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
import { useVideoRect } from "./hooks/useVideoRect";
import { useApp, useDispatch } from "./store";
import { CategoryRail } from "./components/CategoryRail";
import { ChannelList } from "./components/ChannelList";
import { CommandPalette } from "./components/CommandPalette";
import { PlayerChrome } from "./components/PlayerChrome";
import { SettingsModal } from "./components/SettingsModal";
import { SyncStrip } from "./components/SyncStrip";
import { Toasts } from "./components/Toasts";
import { NowCard } from "./components/NowCard";

export function App() {
  const state = useApp();
  const dispatch = useDispatch();
  const dockRef = useRef<HTMLDivElement | null>(null);
  const planeRef = useRef<HTMLDivElement | null>(null);
  const browsing = state.mode === "browse";

  useGlobalKeys(state, dispatch);

  useVideoRect({
    dockRef,
    docked: browsing,
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

      <SyncStrip />

      {browsing ? (
        <div className="browse">
          <CategoryRail />
          <ChannelList />
          <section className="side">
            <div className="side-spacer" />
            {/* The dock is a hole, not a surface: no background, ever. It
                still takes clicks — a transparent element is perfectly
                clickable, and clicking the picture is how you go back to it. */}
            <div
              className="dock"
              ref={dockRef}
              role="button"
              tabIndex={0}
              title="Back to full screen"
              onClick={() => {
                if (state.player.state !== "stopped") {
                  dispatch({ type: "mode", mode: "watch" });
                }
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter" && state.player.state !== "stopped") {
                  dispatch({ type: "mode", mode: "watch" });
                }
              }}
            />
            <NowCard />
          </section>
        </div>
      ) : null}

      {/* Chrome belongs to watch mode. Browsing shows the docked picture and
          the metadata beside it, not a full set of transport controls. */}
      {browsing ? null : <PlayerChrome />}

      {state.paletteOpen ? <CommandPalette /> : null}
      {state.settingsOpen ? <SettingsModal /> : null}

      <Toasts />
    </div>
  );
}
