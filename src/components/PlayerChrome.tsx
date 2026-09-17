// Controls over the video. Everything here is either transparent or a
// translucent gradient: mpv is drawing underneath, and an opaque panel would
// be a hole in the picture.
//
// Click does not pause. That is a VOD idiom and it misfires constantly on
// live TV, where the click you meant for the window lands on the stream.
//
// The order of the bottom row is the user's, and it is not arbitrary:
//
//   play/pause · stop · volume + slider · compressor ·· skip back ·
//   skip forward · live · buffered ··· stack ··· browse … fullscreen
//
// Play and stop lead, because they are the two things anybody reaches for
// without looking, and both sit left of the volume control. Volume and the
// compressor are the audio pair; a gap separates them from transport, so the
// hand does not confuse the two groups. The stacked-layout chevron is in the
// middle of the bar. Fullscreen is last, always, because a control that moves
// is a control you have to look for.
//
// The measured line is not in this row: it is an overlay in the top-right
// corner of the picture, where it was, and where it does not take a control's
// worth of width from a row that already wraps at high zoom.

import { useCallback, useEffect, useRef, useState } from "react";
import { backend } from "../backend";
import { toggleFullscreen } from "../fullscreen";
import { clock, liveState, measuredLine } from "../logic/format";
import { atScreenEdge, initialFade, nextFade, type FadeEvent } from "../logic/fade";
import { volumeAfterWheel, volumeLevel } from "../logic/volume";
import { zoneAt, zoneIsAvailable, zoneSeek } from "../logic/gestures";
import { run, useApp, useDispatch } from "../store";
import {
  Back,
  ChevronDown,
  ChevronUp,
  CompressorOff,
  CompressorOn,
  ExitFullscreen,
  Fullscreen,
  Pause,
  Play,
  SkipBack,
  SkipForward,
  Stop,
} from "./icons";
import { useCanStack } from "../hooks/useCanStack";
import { refreshFullscreen, useFullscreen } from "../hooks/useFullscreen";
import { Scrubber } from "./Scrubber";
import { VideoFeedback, type Feedback } from "./VideoFeedback";
import { VolumeControl } from "./VolumeControl";

/** What the two skip controls move by, and what their icons say. One number,
 *  so the arrow can never claim a different jump from the one it makes. */
const SKIP_SECONDS = 5;

export function PlayerChrome() {
  const { player, nowChannel, stacked, zoom } = useApp();
  const dispatch = useDispatch();
  // Offered only when the window can actually take the layout. Below that the
  // control is absent rather than present and cramped.
  const room = useCanStack(zoom);
  const [fade, setFade] = useState(() => initialFade(performance.now()));
  const fullscreen = useFullscreen();

  const on = useCallback(
    (e: FadeEvent) => setFade((s) => nextFade(s, e, performance.now())),
    [],
  );

  // The idle countdown needs a clock of its own; the state machine only
  // decides, it does not keep time.
  useEffect(() => {
    const t = window.setInterval(() => on({ type: "tick" }), 500);
    return () => window.clearInterval(t);
  }, [on]);

  const volumeRef = useRef(player.volume);
  volumeRef.current = player.volume;

  // The gestures over the picture change things nothing on screen is showing,
  // so each one raises a bubble. The counter is what makes a repeat visible:
  // an identical gesture produces an identical description, and without a
  // changing id React would reuse the element and the animation would not
  // replay.
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const feedbackId = useRef(0);
  const flash = useCallback((f: Omit<Feedback, "id">) => {
    feedbackId.current += 1;
    setFeedback({ ...f, id: feedbackId.current });
  }, []);

  const playing = player.state === "playing";
  const measured = measuredLine(player);
  // The backend decides what counts as live, with hysteresis. The interface
  // must not second-guess it from behindLive, which sawtooths by seconds
  // during ordinary playback — that is precisely the flicker.
  const live = liveState(player.atLiveEdge, player.behindLive);

  /** One place decides what setting a volume means. Reaching for the slider
   *  while muted means "let me hear this", so it unmutes rather than moving a
   *  handle that changes nothing audible. */
  const setVolume = (next: number) => {
    volumeRef.current = next;
    void run(dispatch, "volume", backend.setVolume(next));
    if (player.muted) void run(dispatch, "mute", backend.setMuted(false));
  };

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
      onWheel={(e) => {
        // Ctrl+wheel is application zoom, handled on the window. A zoom is not
        // a volume change, and doing both would be two surprises at once.
        if (e.ctrlKey || e.metaKey) return;
        // Scroll over the video is volume, and it goes above 100 on purpose.
        const next = volumeAfterWheel(volumeRef.current, e.deltaY);
        volumeRef.current = next;
        on({ type: "activity" });
        flash({
          kind: "volume",
          label: `${next}%`,
          level: volumeLevel(next, player.muted),
        });
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

        const box = e.currentTarget.getBoundingClientRect();
        const zone = zoneAt(e.clientX, box.left, box.width);

        if (zone === "middle") {
          void toggleFullscreen()
            .then(refreshFullscreen)
            .catch((err: unknown) =>
              dispatch({ type: "toast", text: `fullscreen: ${String(err)}` }),
            );
          return;
        }

        // Forward at the live edge has nowhere to go. Saying nothing is
        // better than a bubble promising five seconds that do not exist.
        if (!zoneIsAvailable(zone, player.atLiveEdge)) return;

        const by = zoneSeek(zone, SKIP_SECONDS);
        if (by === null) return;
        on({ type: "activity" });
        flash({ kind: zone, label: `${SKIP_SECONDS} seconds`, seconds: SKIP_SECONDS });
        void run(dispatch, "seek", backend.seekRelative(by));
      }}
    >
      <VideoFeedback feedback={feedback} />
      <div className="chrome-top">
        {/* Top-left, where the way out of a full-screen thing lives on every
            device anybody owns. It leaves the player without stopping the
            stream — the same thing Escape does. */}
        <button
          className="ctl chrome-back"
          title="Back to browsing (Escape)"
          aria-label="Back to browsing"
          onClick={() => dispatch({ type: "mode", mode: "browse" })}
        >
          <Back className="i" />
          BACK
        </button>

        <div className="chrome-title">
          <h2>{nowChannel?.cleanName ?? "Nothing playing"}</h2>
          <p>
            {player.state === "opening"
              ? "Opening…"
              : player.state === "failed"
                ? (player.error ?? "Stream failed")
                : (nowChannel?.nowPlaying?.title ?? "no guide data")}
          </p>
        </div>

        {/* The honest numbers, in the corner of the picture. The channel name
            is not evidence of anything — "HARBOUR SPORTS ONE 4K" is 720p. */}
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
              title={
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

        <Scrubber
          player={player}
          channel={nowChannel}
          onPin={(pinned) => on({ type: "pin", pinned })}
        />

        <div
          className="ctl-row"
          onPointerEnter={() => on({ type: "pin", pinned: true })}
          onPointerLeave={() => on({ type: "pin", pinned: false })}
        >
          <button
            className="ctl icon"
            title={playing ? "Pause (Space)" : "Play (Space)"}
            aria-label={playing ? "Pause" : "Play"}
            onClick={() => void run(dispatch, "pause", backend.togglePause())}
          >
            {playing ? <Pause className="i" /> : <Play className="i" />}
          </button>

          {/* A plain square, beside play, left of the volume: the user's
              order. Stopping leaves the player, because a stopped player is
              not a thing to sit looking at. */}
          <button
            className="ctl icon"
            title="Stop"
            aria-label="Stop"
            onClick={() => {
              void run(dispatch, "stop", backend.stop());
              dispatch({ type: "mode", mode: "browse" });
            }}
          >
            <Stop className="i" />
          </button>

          <VolumeControl
            volume={player.volume}
            muted={player.muted}
            onVolume={setVolume}
            onToggleMute={() => void run(dispatch, "mute", backend.setMuted(!player.muted))}
            onActivity={() => on({ type: "activity" })}
          />

          <button
            className={`ctl icon ${player.compressor ? "on" : ""}`}
            title="Dynamic range compressor"
            aria-label="Dynamic range compressor"
            aria-pressed={player.compressor}
            onClick={() =>
              void run(dispatch, "compressor", backend.setCompressor(!player.compressor))
            }
          >
            {player.compressor ? (
              <CompressorOn className="i" />
            ) : (
              <CompressorOff className="i" />
            )}
          </button>

          {/* A gap: audio on one side of it, transport on
              the other. */}
          <span className="ctl-gap" />

          <button
            className="ctl icon"
            title={`Back ${SKIP_SECONDS} seconds (Left)`}
            aria-label={`Back ${SKIP_SECONDS} seconds`}
            onClick={() => void run(dispatch, "seek", backend.seekRelative(-SKIP_SECONDS))}
          >
            <SkipBack className="i" seconds={SKIP_SECONDS} />
          </button>
          <button
            className="ctl icon"
            title={`Forward ${SKIP_SECONDS} seconds (Right)`}
            aria-label={`Forward ${SKIP_SECONDS} seconds`}
            onClick={() => void run(dispatch, "seek", backend.seekRelative(SKIP_SECONDS))}
          >
            <SkipForward className="i" seconds={SKIP_SECONDS} />
          </button>

          {/* One control, not a lamp beside a button: the word is the button,
              and the dot is its state. At the live edge there is nowhere to
              jump to, so it stops being clickable rather than lying. */}
          <button
            className={`ctl live ${live.live ? "at-edge" : ""}`}
            title={live.canJump ? `${live.label} — click to jump to live (L)` : live.label}
            aria-label={live.label}
            disabled={!live.canJump}
            onClick={() => void run(dispatch, "live", backend.seekLive())}
          >
            <span className="live-dot" aria-hidden="true" />
            Live
          </button>

          <span className="time" title="Buffered scrollback">
            {clock(player.bufferedBack)}
          </span>

          <span className="spring" />

          <button
            className="ctl"
            title="Browse without stopping playback (Escape)"
            onClick={() => dispatch({ type: "mode", mode: "browse" })}
          >
            Browse
          </button>

          {/* Last, always — and still last after the row wraps, which is what
              `rightmost` is for. */}
          <button
            className="ctl icon rightmost"
            title={fullscreen ? "Leave fullscreen (F)" : "Fullscreen (F)"}
            aria-label={fullscreen ? "Leave fullscreen" : "Fullscreen"}
            onClick={() =>
              void toggleFullscreen()
                .then(refreshFullscreen)
                .catch((e: unknown) =>
                  dispatch({ type: "toast", text: `fullscreen: ${String(e)}` }),
                )
            }
          >
            {fullscreen ? <ExitFullscreen className="i" /> : <Fullscreen className="i" />}
          </button>
        </div>
      </div>
    </div>
  );
}
