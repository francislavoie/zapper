// The volume slider itself, without the mute button beside it.
//
// Drawn rather than borrowed. A native <input type=range> is a light
// rectangle in several engines whatever `color-scheme` says, it cannot paint
// the boost region differently from the normal one, and the thing it most
// needs to express here is precisely that 0-100 and 100-200 are not the same
// kind of range. See src/logic/volume.ts for the scale.
//
// Split out from VolumeControl because the docked picture wants the same
// slider in a different place — a popover above its control row rather than
// inline beside the button — and two drawings of one control would drift.

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
} from "../logic/volume";

export interface VolumeSliderProps {
  volume: number;
  muted: boolean;
  onVolume: (volume: number) => void;
  /** The chrome fades on idle; touching this control is not idle. */
  onActivity?: () => void;
}

const pct = (n: number) => `${n * 100}%`;

export function VolumeSlider({ volume, muted, onVolume, onActivity }: VolumeSliderProps) {
  const trackRef = useRef<HTMLDivElement | null>(null);
  const dragging = useRef(false);

  const position = positionOfVolume(volume);
  const boosted = isBoosted(volume);
  const label = volumeLabel(volume, muted);

  const commitAt = (clientX: number) => {
    const el = trackRef.current;
    if (!el) return;
    onActivity?.();
    const box = el.getBoundingClientRect();
    if (box.width <= 0) return;
    onVolume(volumeAtPosition((clientX - box.left) / box.width));
  };

  const nudge = (delta: number) => {
    onActivity?.();
    onVolume(Math.min(VOLUME_MAX, Math.max(0, Math.round(volume + delta))));
  };

  // Attached by hand rather than with onWheel, because React registers wheel
  // at the root as passive and a passive listener cannot preventDefault —
  // and without preventDefault a scroll over the slider also scrolls
  // whatever is underneath it.
  const live = useRef({ volume, onVolume, onActivity });
  live.current = { volume, onVolume, onActivity };
  useEffect(() => {
    const el = trackRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      // Ctrl+wheel is application zoom. Leave it alone, which means not
      // stopping propagation either.
      if (e.ctrlKey || e.metaKey) return;
      e.preventDefault();
      // The same gesture over the video is handled further out. Stopping
      // here keeps one notch from being counted twice.
      e.stopPropagation();
      const { volume: current, onVolume: commit, onActivity: poke } = live.current;
      poke?.();
      commit(volumeAfterWheel(current, e.deltaY));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  return (
    <>
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
    </>
  );
}
