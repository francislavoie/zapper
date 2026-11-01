// Volume: the icon that mutes, and the slider beside it.
//
// The slider is VolumeSlider, which the docked picture also uses — there in
// a popover above its control row rather than inline. One drawing of one
// control; see that file for why it is drawn rather than borrowed from the
// platform.

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
import { VolumeSlider } from "./VolumeSlider";

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
        data-tip={muted ? "Unmute" : "Mute"}
        aria-label={muted ? "Unmute" : "Mute"}
        aria-pressed={muted}
      >
        <Volume className="i" level={volumeLevel(volume, muted)} />
      </button>

      <VolumeSlider volume={volume} muted={muted} onVolume={onVolume} onActivity={onActivity} />
    </div>
  );
}
