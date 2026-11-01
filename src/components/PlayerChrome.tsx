// Controls over the video. Everything here is either transparent or a
// translucent gradient: mpv is drawing underneath, and an opaque panel would
// be a hole in the picture.
//
// Click does not pause. That is a VOD idiom and it misfires constantly on
// live TV, where the click you meant for the window lands on the stream.
//
// The bottom bar is TransportBar, which the docked picture uses too — see
// that file for the order of the row and why. What is only here: the way
// back, the title, the measured line, the stacked-layout chevron (in a row
// of its own, centred, above the scrubber), and Browse.
//
// The measured line is not in the bottom row: it is an overlay in the
// top-right corner of the picture, where it was, and where it does not take
// a control's worth of width from a row that already wraps at high zoom.

import { useRef } from "react";
import { backend } from "../backend";
import { toggleFullscreen } from "../fullscreen";
import { endedMessage, startingLine } from "../logic/ending";
import { measuredLine } from "../logic/format";
import { atScreenEdge } from "../logic/fade";
import { volumeAfterWheel } from "../logic/volume";
import { run, useApp, useDispatch } from "../store";
import { Back, ChevronDown, ChevronUp } from "./icons";
import { useCanStack } from "../hooks/useCanStack";
import { useFade } from "../hooks/useFade";
import { refreshFullscreen, useFullscreen } from "../hooks/useFullscreen";
import { useVideoGestures } from "../hooks/useVideoGestures";
import { StreamEnded } from "./StreamEnded";
import { TransportBar } from "./TransportBar";
import { VideoFeedback } from "./VideoFeedback";
import { PictureCorner } from "./PictureCorner";

export function PlayerChrome() {
  const { player, nowChannel, stacked, zoom } = useApp();
  const dispatch = useDispatch();
  // Offered only when the window can actually take the layout. Below that the
  // control is absent rather than present and cramped.
  const room = useCanStack(zoom);
  const [fade, on] = useFade();
  const fullscreen = useFullscreen();

  const volumeRef = useRef(player.volume);
  volumeRef.current = player.volume;

  const measured = measuredLine(player);
  const ended = endedMessage(player);
  const starting = startingLine(player);

  const toggle = () =>
    void toggleFullscreen()
      .then(refreshFullscreen)
      .catch((err: unknown) => dispatch({ type: "toast", text: `fullscreen: ${String(err)}` }));

  const gestures = useVideoGestures({
    atLiveEdge: player.atLiveEdge,
    muted: player.muted,
    onFullscreen: toggle,
    onActivity: () => on({ type: "activity" }),
  });

  return (
    <div
      className={`chrome ${fade.visible ? "" : "faded"}`}
      onPointerMove={(e) => {
        // In fullscreen, reaching for a screen edge means reaching past the
        // picture: get out of the way at once rather than in five seconds.
        if (fullscreen && atScreenEdge(e.clientX, e.clientY, window.innerWidth, window.innerHeight)) {
          on({ type: "edge" });
        } else {
          on({ type: "activity" });
        }
      }}
      onPointerLeave={() => on({ type: "leave" })}
      onClick={(e) => {
        // A click on a paused picture resumes it. Only that way round: a
        // click never pauses (see logic/gestures.ts), so this cannot misfire
        // on live TV — the worst a stray click does is what was wanted.
        if (player.state !== "paused") return;
        if ((e.target as HTMLElement).closest(".chrome-top, .chrome-bottom")) return;
        on({ type: "activity" });
        void run(dispatch, "pause", backend.togglePause());
      }}
      onWheel={(e) => {
        // Ctrl+wheel is application zoom, handled on the window. A zoom is not
        // a volume change, and doing both would be two surprises at once.
        if (e.ctrlKey || e.metaKey) return;
        // Scroll over the video is volume, and it goes above 100 on purpose.
        const next = volumeAfterWheel(volumeRef.current, e.deltaY);
        volumeRef.current = next;
        on({ type: "activity" });
        gestures.flashVolume(next);
        void run(dispatch, "volume", backend.setVolume(next));
      }}
      onContextMenu={(e) => {
        // Only over the picture: a right-click on a control belongs to that
        // control, if it ever grows one.
        if ((e.target as HTMLElement).closest(".chrome-top, .chrome-bottom")) return;
        e.preventDefault();
        dispatch({ type: "videoMenu", at: { x: e.clientX, y: e.clientY } });
      }}
      onDoubleClick={(e) => {
        // Only over the picture. A double-click that lands on the control bar
        // belongs to the control it hit, and seeking as well would be a second
        // thing the viewer did not ask for.
        if ((e.target as HTMLElement).closest(".chrome-top, .chrome-bottom")) return;
        gestures.doubleClick(e.clientX, e.currentTarget.getBoundingClientRect());
      }}
    >
      <VideoFeedback feedback={gestures.feedback} />
      <PictureCorner />
      {/* Why the picture went away, and the way back. Dismissing leaves the
          player, as stopping does. */}
      <StreamEnded onDismissed={() => dispatch({ type: "mode", mode: "browse" })} />
      <div className="chrome-top">
        {/* Top-left, where the way out of a full-screen thing lives on every
            device anybody owns. It leaves the player without stopping the
            stream — the same thing Escape does. */}
        <button
          className="ctl chrome-back"
          data-tip="Back to browsing (Escape)"
          aria-label="Back to browsing"
          onClick={() => dispatch({ type: "mode", mode: "browse" })}
        >
          <Back className="i" />
          BACK
        </button>

        <div className="chrome-title">
          <h2 data-tip-overflow={nowChannel?.name ?? ""}>
            {nowChannel?.cleanName ?? "Nothing playing"}
          </h2>
          <p>
            {starting
              ? starting
              : ended
                ? ended.title
                : (nowChannel?.nowPlaying?.title ?? "no guide data")}
          </p>
        </div>

        {/* The honest numbers, in the corner of the picture. The channel name
            is not evidence of anything — "HARBOUR SPORTS 1 4K" may well be
            720p. */}
        <div className={`measured ${measured ? "" : "show"}`}>
          {measured || "resolution unknown"}
        </div>
      </div>

      <div className="chrome-bottom">
        {/* Centred on the bar, in a slot of its own. It was between two
            springs in the control row, which is only the middle of the bar
            while that row fits on one line — and the windows this control
            appears in are exactly the narrow ones where it wraps. A row of its
            own is the middle of the bar at every width.

            Only when the window can take the layout: taller than wide, with
            half again the picture's height to spare. In anything else the
            result would be a letterbox over three rows of guide, so the
            control is not there at all. */}
        {room ? (
          <div className="chrome-stack-row">
            <button
              className={`ctl icon stack-toggle ${stacked ? "on" : ""}`}
              data-tip={
                stacked
                  ? "Give the whole window back to the picture"
                  : "Put the picture across the top, with the guide below"
              }
              aria-label={stacked ? "Full-window video" : "Video above the guide"}
              aria-pressed={stacked}
              onClick={() => dispatch({ type: "stacked", on: !stacked })}
            >
              {stacked ? <ChevronUp className="i" /> : <ChevronDown className="i" />}
            </button>
          </div>
        ) : null}

        <TransportBar
          onPin={(pinned) => on({ type: "pin", pinned })}
          onActivity={() => on({ type: "activity" })}
          fullscreen={fullscreen}
          onFullscreen={toggle}
          // Stopping leaves the player, because a stopped player is not a
          // thing to sit looking at.
          onStopped={() => dispatch({ type: "mode", mode: "browse" })}
          onBrowse={() => dispatch({ type: "mode", mode: "browse" })}
        />
      </div>
    </div>
  );
}
