// The pieces every Settings page is built from: a page with its title and
// one line on what it covers, titled groups of settings in boxes, and the
// row each setting sits on.
//
// A row is its label and a line of description on the left and its control
// on the right. A value that differs from its default shows a ↺ beside the
// label that puts back that one setting and nothing else. The ↺ keeps its
// place when hidden, so labels do not shift as it comes and goes.

import { useId, useRef, type CSSProperties, type ReactNode } from "react";
import { ResetArrow } from "./icons";

// `heading` rather than `title`: a `title=` anywhere in the components reads
// as the native tooltip the checks forbid, and would be one if it ever
// reached the DOM.

export function SettingsPage({
  heading,
  summary,
  children,
}: {
  heading: string;
  summary?: ReactNode;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <section className="set-page" aria-labelledby={id}>
      <header className="set-page-head">
        <h3 id={id}>{heading}</h3>
        {summary ? <p className="set-summary">{summary}</p> : null}
      </header>
      {children}
    </section>
  );
}

/** A titled box of rows. `bare`: the content draws its own boxes (the
 *  account type's cards). */
export function SettingsGroup({
  heading,
  hint,
  bare = false,
  children,
}: {
  heading: string;
  /** A few words after the title. */
  hint?: string;
  bare?: boolean;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <section className="set-group" role="group" aria-labelledby={id}>
      <h4 id={id} className="set-group-title">
        {heading}
        {hint ? <span className="set-group-hint"> · {hint}</span> : null}
      </h4>
      {bare ? children : <div className="set-card">{children}</div>}
    </section>
  );
}

export interface SettingRowProps {
  label: ReactNode;
  /** The control's id: the label names it, and a click on it reaches it. */
  htmlFor?: string;
  description?: ReactNode;
  /** The value differs from its default, so the ↺ shows. */
  modified?: boolean;
  /** Puts this one setting back. Without it the row has no ↺ at all. */
  onReset?: () => void;
  /** The default, as the ↺'s tip says it: "Reset to 512 MB". */
  resetTo?: string;
  /** "side" (the default): the control on the right. "form": a form row,
   *  the label in a fixed column and the control filling the rest. "stack":
   *  the control under the text, the row's full width. */
  layout?: "side" | "form" | "stack";
  children?: ReactNode;
}

export function SettingRow({
  label,
  htmlFor,
  description,
  modified = false,
  onReset,
  resetTo,
  layout = "side",
  children,
}: SettingRowProps) {
  const row = useRef<HTMLDivElement | null>(null);
  const reset = () => {
    onReset?.();
    // The ↺ hides as it is pressed, which would drop the focus on the floor;
    // it goes to the control that was just put back instead.
    row.current
      ?.querySelector<HTMLElement>(
        ".set-control input, .set-control select, .set-control textarea, .set-control [role='slider'], .set-control [role='switch'], .set-control [aria-checked='true'], .set-control button",
      )
      ?.focus();
  };
  const tip = resetTo ? `Reset to ${resetTo}` : "Reset to the default";
  return (
    <div ref={row} className={`set-row ${layout}`} data-modified={modified || undefined}>
      <div className="set-text">
        <div className="set-head">
          {htmlFor ? (
            <label className="set-label" htmlFor={htmlFor}>
              {label}
            </label>
          ) : (
            <span className="set-label">{label}</span>
          )}
          {onReset ? (
            <button
              type="button"
              className="set-reset"
              aria-label={tip}
              data-tip={tip}
              // Hidden rather than removed, so the label never moves.
              style={{ visibility: modified ? "visible" : "hidden" }}
              onClick={reset}
            >
              <ResetArrow className="i" />
            </button>
          ) : null}
        </div>
        {description ? <div className="set-desc">{description}</div> : null}
      </div>
      {children !== undefined ? <div className="set-control">{children}</div> : null}
    </div>
  );
}

export interface RangeFieldProps {
  id: string;
  min: number;
  max: number;
  step: number;
  value: number;
  onChange: (value: number) => void;
  /** How a value is written: the readout and the two ends. */
  format: (value: number) => string;
  /** The default, marked on the track. */
  mark?: number;
  /** A few words after the readout on what the value means. */
  note?: ReactNode;
}

const fraction = (v: number, min: number, max: number) =>
  max > min ? Math.min(1, Math.max(0, (v - min) / (max - min))) : 0;

/** A slider with what it means beside it: the value with its unit, the two
 *  ends of its range under the track, and the default marked on it.
 *
 *  A native range input underneath, for the keyboard and the label, painted
 *  in the volume slider's shape: a bright enough track, the accent fill up
 *  to the handle. */
export function RangeField({ id, min, max, step, value, onChange, format, mark, note }: RangeFieldProps) {
  const m = mark === undefined ? null : fraction(mark, min, max);
  const style = { "--f": fraction(value, min, max) } as CSSProperties;
  return (
    <div className="range">
      <div className="range-read">
        <output htmlFor={id} className="range-value">
          {format(value)}
        </output>
        {note ? <span className="range-note">{note}</span> : null}
      </div>
      <div className="range-track" style={style}>
        <input
          id={id}
          className="slider"
          type="range"
          min={min}
          max={max}
          step={step}
          value={value}
          aria-valuetext={format(value)}
          onChange={(e) => onChange(Number(e.target.value))}
        />
        {m !== null ? (
          <span className="range-mark" style={{ "--m": m } as CSSProperties} aria-hidden />
        ) : null}
      </div>
      <div className="range-scale" aria-hidden>
        <span>{format(min)}</span>
        {/* The default's own figure under its mark, where there is room for
            it between the two ends. */}
        {m !== null && m > 0.18 && m < 0.82 ? (
          <span className="range-scale-mark" style={{ "--m": m } as CSSProperties}>
            {format(mark as number)}
          </span>
        ) : null}
        <span>{format(max)}</span>
      </div>
    </div>
  );
}

export interface SegmentedOption<T extends string> {
  id: T;
  label: string;
  /** Said on hover: what the choice does. */
  tip?: string;
}

/** One of a few, side by side: a row of buttons that read as one control. */
export function Segmented<T extends string>({
  id,
  value,
  options,
  onChange,
  label,
}: {
  id?: string;
  value: T;
  options: readonly SegmentedOption<T>[];
  onChange: (value: T) => void;
  /** Its accessible name. */
  label: string;
}) {
  return (
    <div id={id} className="segmented" role="radiogroup" aria-label={label}>
      {options.map((o) => (
        <button
          key={o.id}
          type="button"
          role="radio"
          aria-checked={o.id === value}
          className={o.id === value ? "on" : ""}
          data-tip={o.tip}
          onClick={() => onChange(o.id)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}
