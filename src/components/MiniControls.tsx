// The docked picture's controls, and its gestures.
//
// The player's full chrome only exists in watch mode, so while the picture
// was docked there was nothing to press: pausing or muting meant opening the
// player and coming back. The bar here is the player's own TransportBar at
// its compact size — scrubber, scrollback readout and every button, with the
// same tooltips and keys — so the two cannot drift apart. Only Browse is
// left out, and only because the band is where Browse goes.
//
// The gestures are the player's too, through the same hook: scroll is volume,
// a double-click at either edge seeks with a bubble to say so. What differs
// is what a click means, because the band gives the picture a job of its own
// — a single click opens the player (see WatchBand) — and so a double-click
// in the middle goes on past the player to fullscreen.
//
// Drawn over the video deliberately. The slot is a hole through to mpv and
// nothing may sit on it permanently — but controls that appear under the
// pointer and leave again are what that hole is for. This layer itself is
// transparent and covers the slot only to hear the pointer; the bar is
// invisible and untouchable until the pointer is there.

import { useEffect, useLayoutEffect, useRef } from "react";
import { backend } from "../backend";
import { toggleFullscreen } from "../fullscreen";
import { useFade } from "../hooks/useFade";
import { refreshFullscreen } from "../hooks/useFullscreen";
import { useVideoGestures } from "../hooks/useVideoGestures";
import { DOUBLE_CLICK_MS, clickIntent } from "../logic/gestures";
import { volumeAfterWheel } from "../logic/volume";
import { run, useApp, useDispatch } from "../store";
import { TransportBar } from "./TransportBar";
import { VideoFeedback } from "./VideoFeedback";
import { PictureCorner } from "./PictureCorner";

export interface MiniControlsProps {
  /** What a single click on the picture does. Owned by the band, which also
   *  answers Enter and the click on a picture that is not playing yet. */
  onActivate: () => void;
}

export function MiniControls({ onActivate }: MiniControlsProps) {
  const { player, mode } = useApp();
  const dispatch = useDispatch();
  // Hidden until the pointer arrives: a stream starting is not a reason to
  // put a panel over the picture.
  const [fade, on] = useFade(false);
  const rootRef = useRef<HTMLDivElement | null>(null);

  // The band stays mounted, hidden, while the player is open (App.tsx keeps
  // it in an <Activity>). Coming back, the bar must start hidden rather than
  // as it was when the picture was clicked — before paint, so the panel is
  // never seen over the picture for the half second the idle clock needs.
  useLayoutEffect(() => on({ type: "leave" }), [on]);

  // From the docked picture, fullscreen means all the way there rather than
  // to the player and then fullscreen. The mode change is not cosmetic: the
  // video rect is measured from whichever slot the layout offers, and browse
  // offers the corner — so going fullscreen without it leaves a full-screen
  // window with the picture still in the corner of it.
  const toFullscreen = () => {
    dispatch({ type: "fullscreenFrom", mode });
    dispatch({ type: "mode", mode: "watch" });
    void toggleFullscreen()
      .then(refreshFullscreen)
      .catch((err: unknown) => dispatch({ type: "toast", text: `fullscreen: ${String(err)}` }));
  };

  const gestures = useVideoGestures({
    atLiveEdge: player.atLiveEdge,
    muted: player.muted,
    onFullscreen: toFullscreen,
    onActivity: () => on({ type: "activity" }),
  });

  // Scroll over the docked picture is volume, exactly as it is over the
  // full-size one. Attached by hand rather than with onWheel, because React
  // registers wheel at the root as passive and a passive listener cannot
  // preventDefault — without which the list scrolls underneath as well.
  const latest = useRef({ player, gestures, on });
  latest.current = { player, gestures, on };
  useEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      // Ctrl+wheel is application zoom; leave it to the window.
      if (e.ctrlKey || e.metaKey) return;
      const { player: p, gestures: g, on: poke } = latest.current;
      if (p.state === "stopped") return;
      e.preventDefault();
      const next = volumeAfterWheel(p.volume, e.deltaY);
      poke({ type: "activity" });
      // Scrolling up from mute here means "let me hear this", so the bubble
      // shows the speaker that is about to be heard.
      g.flashVolume(next, false);
      void run(dispatch, "volume", backend.setVolume(next));
      if (p.muted) void run(dispatch, "mute", backend.setMuted(false));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [dispatch]);

  // A single click waits to see whether a second is coming. See
  // DOUBLE_CLICK_MS: answering at once would unmount this picture under the
  // second click.
  const pending = useRef<number | null>(null);
  const cancelPending = () => {
    if (pending.current !== null) window.clearTimeout(pending.current);
    pending.current = null;
  };
  useEffect(() => cancelPending, []);

  return (
    <div
      className={`mini-chrome ${fade.visible ? "shown" : ""}`}
      ref={rootRef}
      onPointerMove={() => on({ type: "activity" })}
      onPointerLeave={() => on({ type: "leave" })}
      onClick={(e) => {
        // The band's own click handler is for when this layer is absent.
        e.stopPropagation();
        const intent = clickIntent(e.detail);
        if (intent === "single") {
          cancelPending();
          pending.current = window.setTimeout(() => {
            pending.current = null;
            onActivate();
          }, DOUBLE_CLICK_MS);
        } else if (intent === "double") {
          cancelPending();
          gestures.doubleClick(e.clientX, e.currentTarget.getBoundingClientRect());
        }
      }}
    >
      <VideoFeedback feedback={gestures.feedback} />
      <PictureCorner docked />

      {/* Every click inside the bar belongs to the control it hit. Without
          stopping it here, pausing would also count as a click on the
          picture and open the player. */}
      <div
        className="mini-controls"
        onClick={(e) => e.stopPropagation()}
        onContextMenu={(e) => e.stopPropagation()}
      >
        <TransportBar
          compact
          onPin={(pinned) => on({ type: "pin", pinned })}
          onActivity={() => on({ type: "activity" })}
          fullscreen={false}
          onFullscreen={toFullscreen}
          hintWithin={rootRef}
        />
      </div>
    </div>
  );
}
