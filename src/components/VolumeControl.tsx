// Volume: the icon that mutes, and a real slider beside it.
//
// The slider is drawn rather than borrowed. A native <input type=range> is a
// light rectangle in several engines whatever `color-scheme` says, it cannot
// paint the boost region differently from the normal one, and the thing it
// most needs to express here is precisely that 0-100 and 100-200 are not the
// same kind of range. See src/logic/volume.ts for the scale.

import { useEffect, useRef } from "react";
import {
  UNITY_FRACTION,
  VOLUME_MAX,
  VOLUME_STEP,
  VOLUME_UNITY,
  isBoosted,
  positionOfVolume,
  volumeAfterWheel,
  volumeAtPosition,
  volumeLabel,
  volumeLevel,
} from "../logic/volume";
import { Volume } from "./icons";

export interface VolumeControlProps {
  volume: number;
  muted: boolean;
  /** Commits a new volume. The caller also decides what that means for mute —
   *  reaching for the slider while muted means "let me hear this". */
  onVolume: (volume: number) => void;
  onToggleMute: () => void;
  /** The chrome fades on idle; touching this control is not idle. */
  onActivity?: () => void;
}

const pct = (n: number) => `${n * 100}%`;

export function VolumeControl({
  volume,
  muted,
  onVolume,
  onToggleMute,
  onActivity,
}: VolumeControlProps) {
  const rootRef = useRef<HTMLDivElement | null>(null);
  const trackRef = useRef<HTMLDivElement | null>(null);
  const dragging = useRef(false);

  // The wheel listener is attached by hand rather than with onWheel, because
  // React registers wheel at the root as passive and a passive listener cannot
  // preventDefault — and without preventDefault a scroll over the slider also
  // scrolls whatever is underneath it.
  const live = useRef({ volume, onVolume, onActivity });
  live.current = { volume, onVolume, onActivity };

  useEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      // Ctrl+wheel is application zoom. Leave it alone and let it reach the
      // window, which means not stopping propagation either.
      if (e.ctrlKey || e.metaKey) return;
      e.preventDefault();
      // The same gesture over the video is handled by the chrome. Stopping
      // here keeps one notch from being counted twice.
      e.stopPropagation();
      const { volume: current, onVolume: commit, onActivity: poke } = live.current;
      poke?.();
      // Deliberately the same function the video surface uses: two wheel
      // handlers with two step sizes is the kind of thing nobody can name and
      // everybody can feel.
      commit(volumeAfterWheel(current, e.deltaY));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  // Muting does not move the handle. The slider goes on showing the value
  // playback will come back to — greyed, so it is clear nothing is coming out
  // of it right now — and `aria-valuenow` says the same number the handle is
  // sitting on, which it would not if one of them dropped to zero.
  const position = positionOfVolume(volume);
  const boosted = isBoosted(volume);
  const label = volumeLabel(volume, muted);

  const commitAt = (clientX: number) => {
    const el = trackRef.current;
    if (!el) return;
    const box = el.getBoundingClientRect();
    if (box.width <= 0) return;
    onActivity?.();
    onVolume(volumeAtPosition((clientX - box.left) / box.width));
  };

  const nudge = (delta: number) => {
    onActivity?.();
    onVolume(Math.min(VOLUME_MAX, Math.max(0, Math.round(volume + delta))));
  };

  return (
    <div className={`volume ${muted ? "muted" : ""}`} ref={rootRef}>
      <button
        className="ctl icon"
        onClick={() => {
          onActivity?.();
          onToggleMute();
        }}
        title={muted ? "Unmute" : "Mute"}
        aria-label={muted ? "Unmute" : "Mute"}
        aria-pressed={muted}
      >
        <Volume className="i" level={volumeLevel(volume, muted)} />
      </button>

      <div
        className="vol"
        ref={trackRef}
        role="slider"
        tabIndex={0}
        aria-label="Volume"
        aria-valuemin={0}
        aria-valuemax={VOLUME_MAX}
        aria-valuenow={Math.round(volume)}
        aria-valuetext={label}
        onPointerDown={(e) => {
          if (e.button !== 0) return;
          dragging.current = true;
          // Capture, so a drag that leaves the control still moves the handle
          // instead of stopping dead at the edge.
          e.currentTarget.setPointerCapture(e.pointerId);
          commitAt(e.clientX);
        }}
        onPointerMove={(e) => {
          if (dragging.current) commitAt(e.clientX);
        }}
        onPointerUp={(e) => {
          dragging.current = false;
          e.currentTarget.releasePointerCapture(e.pointerId);
        }}
        onPointerCancel={() => {
          dragging.current = false;
        }}
        onKeyDown={(e) => {
          const step = e.shiftKey ? 1 : VOLUME_STEP;
          switch (e.key) {
            case "ArrowRight":
            case "ArrowUp":
              nudge(step);
              break;
            case "ArrowLeft":
            case "ArrowDown":
              nudge(-step);
              break;
            case "Home":
              onVolume(0);
              break;
            case "End":
              onVolume(VOLUME_MAX);
              break;
            case "PageUp":
              nudge(VOLUME_STEP * 4);
              break;
            case "PageDown":
              nudge(-VOLUME_STEP * 4);
              break;
            default:
              return;
          }
          e.preventDefault();
          // The window's own arrow keys seek the stream. A focused slider owns
          // its arrows; React's listener is on the root, so the native event
          // has to be stopped there or it carries on up to the window.
          e.nativeEvent.stopPropagation();
        }}
      >
        <div className="vol-track">
          {/* The boost stretch is marked on the track itself, so it is visible
              before the handle ever gets there. */}
          <div className="vol-zone boost" style={{ left: pct(UNITY_FRACTION) }} />
          <div className="vol-fill" style={{ width: pct(Math.min(position, UNITY_FRACTION)) }} />
          {boosted ? (
            <div
              className="vol-fill over"
              style={{
                left: pct(UNITY_FRACTION),
                width: pct(position - UNITY_FRACTION),
              }}
            />
          ) : null}
          <div className="vol-unity" style={{ left: pct(UNITY_FRACTION) }} />
          <div className={`vol-handle ${boosted ? "over" : ""}`} style={{ left: pct(position) }} />
        </div>
      </div>

      <span
        className={`vol-readout ${boosted && !muted ? "over" : ""}`}
        title={boosted ? `Boosted to ${Math.round(volume)}% (above ${VOLUME_UNITY}% is software gain)` : undefined}
      >
        {label}
      </span>
    </div>
  );
}
