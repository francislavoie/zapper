// The pieces every account in the band is built from (WatchBand decides which
// accounts it shows; logic/band.ts says where). The band has three parts:
//
//   thumb · main (kicker, title, facts, plot, the action row) · side column
//
// The main column's text scrolls when there is more of it than fits; its
// action row never does. A wide band adds a credits column (container query
// on .watch-info); a narrow one folds the side column into one line under
// the main one.

import type { MouseEvent, ReactNode } from "react";
import { useDispatch } from "../store";
import { Poster } from "./Poster";
import { Return } from "./icons";

/** The main column: `thumb`, then the text with the actions at its foot,
 *  then (wide bands only) `credits`. */
export function BandMain({
  thumb,
  kicker,
  children,
  actions,
  credits,
  className = "",
}: {
  thumb?: ReactNode;
  /** Said above the title while the band tells of two things. */
  kicker?: ReactNode;
  children: ReactNode;
  actions: ReactNode;
  credits?: ReactNode;
  className?: string;
}) {
  return (
    <div className={`band-main ${className}`}>
      {thumb}
      <div className="band-text">
        {kicker}
        <div className="watch-body">{children}</div>
        <div className="watch-actions">{actions}</div>
      </div>
      {credits}
    </div>
  );
}

/** A poster at the band's full height, with how far in it is along its foot
 *  when something of it is playing. */
export function BandPoster({
  url,
  name,
  progress = null,
}: {
  url: string | null;
  name: string;
  progress?: number | null;
}) {
  return (
    <span className="band-thumb" aria-hidden>
      <Poster url={url} name={name} className="poster band-thumb-img" />
      {progress !== null ? (
        <span className="band-thumb-bar">
          <span style={{ width: `${Math.round(progress * 100)}%` }} />
        </span>
      ) : null}
    </span>
  );
}

/** A channel's logo on a square tile; its initial where it has none. */
export function BandLogo({ url, name }: { url: string | null; name: string }) {
  return (
    <span className="band-thumb logo" aria-hidden>
      <Poster url={url} name={name} className="poster band-logo-img" />
    </span>
  );
}

/** "Selected", "Now playing · live", "Now playing": which of the two this is. */
export function Kicker({ what }: { what: "selected" | "live" | "playing" }) {
  if (what === "selected") return <p className="band-kicker sel">Selected</p>;
  return (
    <p className="band-kicker dotted">
      <span className={`band-dot ${what === "live" ? "live" : ""}`} aria-hidden />
      {what === "live" ? "Now playing · live" : "Now playing"}
    </p>
  );
}

/** The side column. With `swap`, it is the other of two things the band
 *  tells of, and a click anywhere on it (but on its own buttons) swaps them:
 *  its ↩ Show is the way there from the keyboard. */
export function BandSide({
  kind,
  label,
  swap = false,
  children,
  actions,
}: {
  kind: "now" | "context";
  label: string;
  swap?: boolean;
  children: ReactNode;
  actions?: ReactNode;
}) {
  const dispatch = useDispatch();
  const onClick = swap
    ? (e: MouseEvent<HTMLElement>) => {
        if ((e.target as HTMLElement).closest("button, a, [role=button]")) return;
        dispatch({ type: "bandSwap" });
      }
    : undefined;
  return (
    <aside className={`band-side ${kind} ${swap ? "swaps" : ""}`} aria-label={label} onClick={onClick}>
      {children}
      <span className="band-grow" />
      {actions ? <div className="watch-actions">{actions}</div> : null}
    </aside>
  );
}

/** ↩ Show: the other of the two to the front of the band. */
export function SwapButton({ label }: { label: string }) {
  const dispatch = useDispatch();
  return (
    <button className="ctl band-swap" aria-label={label} onClick={() => dispatch({ type: "bandSwap" })}>
      <Return className="i" /> Show
    </button>
  );
}

/** A bar and what it means, side by side ("34m left", "23:10 / 52:00"). */
export function ProgressLine({ fraction, label }: { fraction: number; label?: string }) {
  return (
    <div className="watch-prog-line">
      <div className="watch-progress">
        <span style={{ width: `${Math.max(0, Math.min(1, fraction)) * 100}%` }} />
      </div>
      {label ? <span className="watch-left">{label}</span> : null}
    </div>
  );
}

/** Wide bands only: who made it and who is in it, in a column of its own
 *  rather than a clipped line. Nothing when there is nothing to list. */
export function Credits({ items }: { items: readonly (readonly [string, string])[] }) {
  const shown = items.filter(([, v]) => v !== "");
  if (shown.length === 0) return null;
  return (
    <dl className="band-credits">
      {shown.map(([k, v]) => (
        <div key={k}>
          <dt>{k}</dt>
          <dd data-tip-overflow="">{v}</dd>
        </div>
      ))}
    </dl>
  );
}
