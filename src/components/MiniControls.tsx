// Transport for the docked picture.
//
// The player's full chrome only exists in watch mode, so while the picture
// was docked there was nothing to press: pausing or muting meant opening the
// player and coming back.
//
// The same buttons in the same order as the player bar, built from the same
// `.ctl` classes and the same icons — it is the same control surface at a
// smaller size, not a second one with its own ideas. Two things the bar has
// are left out, and only because they do not fit in a 22rem box: the volume
// slider, whose button remains as mute, and Browse, which is where you
// already are.
//
// Drawn over the video deliberately. The slot is a hole through to mpv and
// nothing may sit on it permanently — but controls that appear under the
// pointer and leave again are what that hole is for.

import { useEffect, useRef, useState } from "react";
import { backend } from "../backend";
import { toggleFullscreen } from "../fullscreen";
import { refreshFullscreen } from "../hooks/useFullscreen";
import { liveState } from "../logic/format";
import { run, useApp, useDispatch } from "../store";
import {
  CompressorOff,
  CompressorOn,
  Fullscreen,
  Pause,
  Play,
  SkipBack,
  SkipForward,
  Stop,
  Volume,
} from "./icons";
import { VolumeSlider } from "./VolumeSlider";

/** The same jump the player bar makes, from the same constant's value. */
const SKIP_SECONDS = 5;

export function MiniControls({ volumeNudge = 0 }: { volumeNudge?: number }) {
  const { player, mode } = useApp();
  const dispatch = useDispatch();
  const playing = player.state === "playing";
  const live = liveState(player.atLiveEdge, player.behindLive);

  // The slider lives above the row rather than in it: the row is already
  // eight controls wide in a 22rem box, and the slider is the one of them
  // that needs length to be worth having.
  //
  // Shown on hover of the volume group, and for a moment after a scroll
  // anywhere over the picture — that gesture changes a number with nothing
  // on screen to show it, so it brings up the thing that does.
  const [nudged, setNudged] = useState(false);
  useEffect(() => {
    if (volumeNudge === 0) return;
    setNudged(true);
    const t = window.setTimeout(() => setNudged(false), 1400);
    return () => window.clearTimeout(t);
  }, [volumeNudge]);

  const setVolume = (next: number) => {
    void run(dispatch, "volume", backend.setVolume(next));
    // Reaching for the slider while muted means "let me hear this".
    if (player.muted) void run(dispatch, "mute", backend.setMuted(false));
  };

  // The slot behind these is click-to-open-the-player. Every button has to
  // stop that, or pausing would also throw you into the player.
  const act = (e: React.MouseEvent, f: () => void) => {
    e.stopPropagation();
    f();
  };

  return (
    <div
      className="mini-controls"
      onDoubleClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.stopPropagation()}
    >
      <button
        className="ctl icon"
        title={playing ? "Pause (Space)" : "Play (Space)"}
        aria-label={playing ? "Pause" : "Play"}
        onClick={(e) => act(e, () => void run(dispatch, "pause", backend.togglePause()))}
      >
        {playing ? <Pause className="i" /> : <Play className="i" />}
      </button>

      <button
        className="ctl icon"
        title="Stop"
        aria-label="Stop"
        onClick={(e) => act(e, () => void run(dispatch, "stop", backend.stop()))}
      >
        <Stop className="i" />
      </button>

      <span className={`mini-volume ${nudged ? "open" : ""}`}>
        <button
          className="ctl icon"
          title={player.muted ? "Unmute" : "Mute"}
          aria-label={player.muted ? "Unmute" : "Mute"}
          aria-pressed={player.muted}
          onClick={(e) =>
            act(e, () => void run(dispatch, "mute", backend.setMuted(!player.muted)))
          }
        >
          <Volume className="i" level={player.muted ? "muted" : "high"} />
        </button>

        {/* Two elements, and the outer one is the reason this is usable.
            It reaches from the button up to the panel with nothing between,
            so moving the pointer towards the slider never crosses a gap —
            cross one and the hover is lost and the thing being reached for
            vanishes. The visible panel is the inner element. */}
        <span className="mini-volume-pop">
          <span
            className="mini-volume-panel volume"
            onClick={(e) => e.stopPropagation()}
            onDoubleClick={(e) => e.stopPropagation()}
          >
            <VolumeSlider volume={player.volume} muted={player.muted} onVolume={setVolume} />
          </span>
        </span>
      </span>

      <button
        className={`ctl icon ${player.compressor ? "on" : ""}`}
        title="Dynamic range compressor"
        aria-label="Dynamic range compressor"
        aria-pressed={player.compressor}
        onClick={(e) =>
          act(
            e,
            () =>
              void run(dispatch, "compressor", backend.setCompressor(!player.compressor)),
          )
        }
      >
        {player.compressor ? <CompressorOn className="i" /> : <CompressorOff className="i" />}
      </button>

      {/* The same gap the bar has: audio on one side, transport on the other. */}
      <span className="ctl-gap" />

      <button
        className="ctl icon"
        title={`Back ${SKIP_SECONDS} seconds (Left)`}
        aria-label={`Back ${SKIP_SECONDS} seconds`}
        onClick={(e) =>
          act(e, () => void run(dispatch, "seek", backend.seekRelative(-SKIP_SECONDS)))
        }
      >
        <SkipBack className="i" seconds={SKIP_SECONDS} />
      </button>

      <button
        className="ctl icon"
        title={`Forward ${SKIP_SECONDS} seconds (Right)`}
        aria-label={`Forward ${SKIP_SECONDS} seconds`}
        onClick={(e) =>
          act(e, () => void run(dispatch, "seek", backend.seekRelative(SKIP_SECONDS)))
        }
      >
        <SkipForward className="i" seconds={SKIP_SECONDS} />
      </button>

      {/* The word as well as the dot, as on the player bar: the dot alone
          says something is red, not what it is. Smaller here, because the
          row has a fifth of the width to spend. */}
      <button
        className={`ctl live mini-live ${live.live ? "at-edge" : ""}`}
        title={live.canJump ? `${live.label} — jump to live (L)` : live.label}
        aria-label={live.label}
        disabled={!live.canJump}
        onClick={(e) => act(e, () => void run(dispatch, "live", backend.seekLive()))}
      >
        <span className="live-dot" aria-hidden="true" />
        Live
      </button>

      {/* Last, like fullscreen on the bar, and the same control: from the
          docked picture that means all the way to fullscreen rather than to
          the player and then fullscreen. Clicking the picture is still the
          way to the player, so nothing is lost by spending this button on
          the further destination.

          The mode change is not cosmetic. The video rect is measured from
          whichever slot the layout offers, and browse offers the corner —
          so going fullscreen without this leaves a full-screen window with
          the picture still in the corner of it. */}
      <button
        className="ctl icon rightmost"
        title="Fullscreen (F)"
        aria-label="Fullscreen"
        onClick={(e) =>
          act(e, () => {
            dispatch({ type: "fullscreenFrom", mode });
            dispatch({ type: "mode", mode: "watch" });
            void toggleFullscreen()
              .then(refreshFullscreen)
              .catch((err: unknown) =>
                dispatch({ type: "toast", text: `fullscreen: ${String(err)}` }),
              );
          })
        }
      >
        <Fullscreen className="i" />
      </button>
    </div>
  );
}
