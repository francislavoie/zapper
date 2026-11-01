// An on/off switch: a `role="switch"` button with `aria-checked`, for every
// yes-or-no setting. A checkbox says "include this"; a switch says "this is
// on", which is what these are, and an "on"/"off" button had to be read to
// know which state it was in.
//
// Space and Enter are taken on key down and stopped there. The window's own
// shortcuts listen for Space (play/pause) further out, and React's listener
// is on the root, so the native event is stopped too or the same press would
// pause the stream as well as flip the switch.

import type { KeyboardEvent } from "react";

export interface SwitchProps {
  checked: boolean;
  onChange: (next: boolean) => void;
  /** The button's id, for a row's `<label htmlFor>`. */
  id?: string;
  /** Its accessible name, when no label points at it. */
  label?: string;
  disabled?: boolean;
}

export function Switch({ checked, onChange, id, label, disabled = false }: SwitchProps) {
  const keys = (e: KeyboardEvent<HTMLButtonElement>) => {
    if (e.key !== " " && e.key !== "Enter") return;
    e.preventDefault();
    e.nativeEvent.stopPropagation();
    if (!e.repeat && !disabled) onChange(!checked);
  };
  return (
    <button
      id={id}
      type="button"
      role="switch"
      className="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      onKeyDown={keys}
      // Space would click again on key up; the press is already handled.
      onKeyUp={(e) => {
        if (e.key === " ") e.preventDefault();
      }}
    >
      <span className="switch-knob" aria-hidden />
    </button>
  );
}
