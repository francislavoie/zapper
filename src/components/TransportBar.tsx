// The transport: the scrubber, and the row of controls under it. One
// component for both places a picture has controls — the player's bottom bar
// and the docked picture in the band — so the two are the same control
// surface at two sizes rather than two surfaces that drift apart.
//
// The order of the row is not arbitrary:
//
//   play/pause · stop · volume + slider · compressor ·· skip back ·
//   skip forward · live · buffered ··· browse … fullscreen
//
// Play and stop lead, because they are the two things anybody reaches for
// without looking, and both sit left of the volume control. Volume and the
// compressor are the audio pair; a gap separates them from transport, so the
// hand does not confuse the two groups. Fullscreen is last, always, because a
// control that moves is a control you have to look for.
//
// `compact` is the docked picture, about 22rem wide. Same controls, same
// tooltips, same keys; what changes is size, which the stylesheet does from
// `.mini-controls`, and two things the width forces:
//
//  - the volume slider is a panel above its button rather than inline, since
//    the slider is the one control that needs length to be worth having;
//  - the scrubber's captions are said in fewer words.
//
// Browse is the caller's to offer (`onBrowse`) and the docked picture does
// not: "browse without stopping" is where you already are.

import type { RefObject } from "react";
import { backend } from "../backend";
import { duration, liveState } from "../logic/format";
import { SKIP_SECONDS } from "../logic/gestures";
import { run, useApp, useDispatch } from "../store";
import {
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
import { Scrubber } from "./Scrubber";
import { VolumeControl } from "./VolumeControl";

export interface TransportBarProps {
  /** The docked picture's size. */
  compact?: boolean;
  /** The bar fades on idle. A pointer over a control holds it open. */
  onPin: (pinned: boolean) => void;
  /** Touching a control is not idle. */
  onActivity: () => void;
  /** Whether the window is fullscreen now, which turns the last button into
   *  the way out. */
  fullscreen: boolean;
  /** The last button. What fullscreen means is the caller's: the player
   *  toggles it; the docked picture goes through the player to get there. */
  onFullscreen: () => void;
  /** After Stop has stopped the stream. The player leaves itself, because a
   *  stopped player is not a thing to sit looking at; the band stays. */
  onStopped?: () => void;
  /** Offers "Browse" when given. */
  onBrowse?: () => void;
  /** The picture the scrubber's preview has to stay inside; see Scrubber. */
  hintWithin?: RefObject<HTMLElement | null>;
}

export function TransportBar({
  compact = false,
  onPin,
  onActivity,
  fullscreen,
  onFullscreen,
  onStopped,
  onBrowse,
  hintWithin,
}: TransportBarProps) {
  const { player, nowChannel } = useApp();
  const dispatch = useDispatch();

  const playing = player.state === "playing";
  // The backend decides what counts as live, with hysteresis. The interface
  // must not second-guess it from behindLive, which sawtooths by seconds
  // during ordinary playback — that is precisely the flicker.
  const live = liveState(player.atLiveEdge, player.behindLive);

  /** One place decides what setting a volume means. Reaching for the slider
   *  while muted means "let me hear this", so it unmutes rather than moving a
   *  handle that changes nothing audible. */
  const setVolume = (next: number) => {
    void run(dispatch, "volume", backend.setVolume(next));
    if (player.muted) void run(dispatch, "mute", backend.setMuted(false));
  };

  return (
    <>
      <Scrubber
        player={player}
        channel={nowChannel}
        onPin={onPin}
        compact={compact}
        hintWithin={hintWithin}
      />

      <div
        className="ctl-row"
        onPointerEnter={() => onPin(true)}
        onPointerLeave={() => onPin(false)}
      >
        <button
          className="ctl icon"
          data-tip={playing ? "Pause (Space)" : "Play (Space)"}
          aria-label={playing ? "Pause" : "Play"}
          onClick={() => void run(dispatch, "pause", backend.togglePause())}
        >
          {playing ? <Pause className="i" /> : <Play className="i" />}
        </button>

        {/* A plain square, beside play, left of the volume. */}
        <button
          className="ctl icon"
          data-tip="Stop"
          aria-label="Stop"
          onClick={() => {
            void run(dispatch, "stop", backend.stop());
            onStopped?.();
          }}
        >
          <Stop className="i" />
        </button>

        <VolumeControl
          volume={player.volume}
          muted={player.muted}
          onVolume={setVolume}
          onToggleMute={() => void run(dispatch, "mute", backend.setMuted(!player.muted))}
          onActivity={onActivity}
          popover={compact}
        />

        <button
          className={`ctl icon ${player.compressor ? "on" : ""}`}
          data-tip="Dynamic range compressor"
          aria-label="Dynamic range compressor"
          aria-pressed={player.compressor}
          onClick={() =>
            void run(dispatch, "compressor", backend.setCompressor(!player.compressor))
          }
        >
          {player.compressor ? <CompressorOn className="i" /> : <CompressorOff className="i" />}
        </button>

        {/* Audio on one side of the gap, transport on the other, so the hand
            does not confuse the two groups. */}
        <span className="ctl-gap" />

        <button
          className="ctl icon"
          data-tip={`Back ${SKIP_SECONDS} seconds (Left)`}
          aria-label={`Back ${SKIP_SECONDS} seconds`}
          onClick={() => void run(dispatch, "seek", backend.seekRelative(-SKIP_SECONDS))}
        >
          <SkipBack className="i" seconds={SKIP_SECONDS} />
        </button>
        <button
          className="ctl icon"
          data-tip={`Forward ${SKIP_SECONDS} seconds (Right)`}
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
          data-tip={live.canJump ? `${live.label} — click to jump to live (L)` : live.label}
          aria-label={live.label}
          disabled={!live.canJump}
          onClick={() => void run(dispatch, "live", backend.seekLive())}
        >
          <span className="live-dot" aria-hidden="true" />
          Live
        </button>

        <span className="time" data-tip="Buffered scrollback">
          {duration(player.bufferedBack)}
        </span>

        <span className="spring" />

        {onBrowse ? (
          <button
            className="ctl"
            data-tip="Browse without stopping playback (Escape)"
            onClick={onBrowse}
          >
            Browse
          </button>
        ) : null}

        {/* Last, always — and still last after the row wraps, which is what
            `rightmost` is for. */}
        <button
          className="ctl icon rightmost"
          data-tip={fullscreen ? "Leave fullscreen (F)" : "Fullscreen (F)"}
          aria-label={fullscreen ? "Leave fullscreen" : "Fullscreen"}
          onClick={onFullscreen}
        >
          {fullscreen ? <ExitFullscreen className="i" /> : <Fullscreen className="i" />}
        </button>
      </div>
    </>
  );
}
