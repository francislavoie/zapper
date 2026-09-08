// Controls over the video. Everything here is either transparent or a
// translucent gradient: mpv is drawing underneath, and an opaque panel would
// be a hole in the picture.
//
// Click does not pause. That is a VOD idiom and it misfires constantly on
// live TV, where the click you meant for the window lands on the stream.

import { useCallback, useEffect, useRef, useState } from "react";
import { backend } from "../backend";
import { toggleFullscreen } from "../fullscreen";
import { clock, liveOffset, measuredLine } from "../logic/format";
import { atScreenEdge, initialFade, nextFade, type FadeEvent } from "../logic/fade";
import { volumeAfterWheel } from "../logic/volume";
import { run, useApp, useDispatch } from "../store";
import { Scrubber } from "./Scrubber";

export function PlayerChrome() {
  const { player, nowChannel } = useApp();
  const dispatch = useDispatch();
  const [fade, setFade] = useState(() => initialFade(performance.now()));
  const [fullscreen, setFullscreen] = useState(false);

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

  useEffect(() => {
    const sync = () => setFullscreen(document.fullscreenElement !== null);
    document.addEventListener("fullscreenchange", sync);
    return () => document.removeEventListener("fullscreenchange", sync);
  }, []);

  const volumeRef = useRef(player.volume);
  volumeRef.current = player.volume;

  const playing = player.state === "playing";
  const measured = measuredLine(player);

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
        // Scroll over the video is volume, and it goes above 100 on purpose.
        const next = volumeAfterWheel(volumeRef.current, e.deltaY);
        volumeRef.current = next;
        on({ type: "activity" });
        void run(dispatch, "volume", backend.setVolume(next));
      }}
    >
      <div className="chrome-top">
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
        {/* The honest numbers. The channel name is not evidence of anything. */}
        <div className={`measured ${measured ? "" : "show"}`}>
          {measured || "resolution unknown"}
        </div>
      </div>

      <div className="chrome-bottom">
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
            className="ctl"
            onClick={() => void run(dispatch, "pause", backend.togglePause())}
          >
            {playing ? "❚❚" : "▶"}
          </button>
          <button
            className="ctl"
            title="Back 5 seconds"
            onClick={() => void run(dispatch, "seek", backend.seekRelative(-5))}
          >
            ‹ 5s
          </button>
          <button
            className="ctl"
            title="Forward 5 seconds"
            onClick={() => void run(dispatch, "seek", backend.seekRelative(5))}
          >
            5s ›
          </button>
          <button
            className={`ctl ${player.behindLive < 1 ? "on" : ""}`}
            title="Jump to the live edge (L)"
            onClick={() => void run(dispatch, "live", backend.seekLive())}
          >
            ⇥
          </button>

          <span className="time">{liveOffset(player.behindLive)}</span>
          <span className="time" title="Buffered scrollback">
            {clock(player.bufferedBack)}
          </span>

          <span className="spring" />

          <button
            className={`ctl ${player.compressor ? "on" : ""}`}
            title="Dynamic range compressor"
            onClick={() =>
              void run(dispatch, "compressor", backend.setCompressor(!player.compressor))
            }
          >
            COMP
          </button>
          <button
            className={`ctl ${player.muted ? "on" : ""}`}
            onClick={() => void run(dispatch, "mute", backend.setMuted(!player.muted))}
          >
            {player.muted ? "muted" : `${player.volume}%`}
          </button>
          <button
            className="ctl"
            title="Fullscreen (F)"
            onClick={() =>
              void toggleFullscreen().catch((e: unknown) =>
                dispatch({ type: "toast", text: `fullscreen: ${String(e)}` }),
              )
            }
          >
            ⛶
          </button>
          <button
            className="ctl"
            title="Browse without stopping playback (Escape)"
            onClick={() => dispatch({ type: "mode", mode: "browse" })}
          >
            Browse
          </button>
          <button
            className="ctl ghost"
            onClick={() => {
              void run(dispatch, "stop", backend.stop());
              dispatch({ type: "mode", mode: "browse" });
            }}
          >
            Stop
          </button>
        </div>
      </div>
    </div>
  );
}
