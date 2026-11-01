// Volume: the icon that mutes, and the slider that goes with it.
//
// Inline on the player bar. On the docked picture's bar (`popover`) it is
// inline too, at the same length — in the row or on a line of its own, as
// the picture's width allows — and on the smallest pictures sits in a small
// panel above the button instead, because the slider is the one control
// that needs length to be worth having. One mute button and one
// slider either way; see VolumeSlider for why the slider is drawn rather than
// borrowed from the platform.

import { useEffect, useRef } from "react";
import { volumeAfterWheel, volumeLevel } from "../logic/volume";
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
  /** The slider in a panel above the button, shown on hover or keyboard
   *  focus, rather than inline beside it. */
  popover?: boolean;
}

export function VolumeControl({
  volume,
  muted,
  onVolume,
  onToggleMute,
  onActivity,
  popover = false,
}: VolumeControlProps) {
  const rootRef = useRef<HTMLDivElement | null>(null);

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

  const slider = (
    <VolumeSlider volume={volume} muted={muted} onVolume={onVolume} onActivity={onActivity} />
  );

  return (
    <div className={`volume ${muted ? "muted" : ""}${popover ? " mini-volume" : ""}`} ref={rootRef}>
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

      {popover ? (
        <>
          {/* Inline when the bar has room for it, which the stylesheet
              decides from the bar's width (`@container mini`): a slider you
              can see is better than one you have to find. Only one of the
              two is ever displayed, so only one is in the tab order and the
              accessibility tree. */}
          <span className="mini-volume-inline">{slider}</span>
          {/* Two elements, and the outer one is the reason this is usable. It
              reaches from the button up to the panel with nothing between, so
              moving the pointer towards the slider never crosses a gap —
              cross one and the hover is lost and the thing being reached for
              vanishes. The visible panel is the inner element. */}
          <span className="mini-volume-pop">
            <span className="mini-volume-panel">{slider}</span>
          </span>
        </>
      ) : (
        slider
      )}
    </div>
  );
}
