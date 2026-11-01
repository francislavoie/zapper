// A channel number being typed, drawn the way a set draws it: the digits so
// far, a blank for each that could still come, and the channel the number
// names as it stands — what Enter would tune. Then, for a moment, the channel
// it tuned, or that there is no such number.
//
// Two places draw it. Over the picture while watching, in its corner, which
// is where a television puts it; as a chip floating over the list or the
// guide while browsing, where the docked picture is too small a stage for it
// and the eye is on the list anyway. Both read the one entry in the store.

import { useApp } from "../store";
import type { ZapView } from "../logic/zap";

export function ZapEntry({ zap }: { zap: ZapView }) {
  const said =
    zap.status === "miss" ? `No channel ${zap.label}` : (zap.name ?? "");
  return (
    <div className={`zap zap-${zap.status}`} role="status" aria-live="polite">
      <span className="zap-digits" aria-label={`Channel ${zap.label}`}>
        {zap.cells.map((c, i) => (
          <span key={i} className={c === "_" ? "zap-blank" : undefined}>
            {c}
          </span>
        ))}
      </span>
      {said ? (
        <span className="zap-name" data-tip-overflow={said}>
          {said}
        </span>
      ) : null}
    </div>
  );
}

/** The entry while browsing: over the views, not over the docked picture. */
export function ZapChip() {
  const { zap } = useApp();
  if (!zap) return null;
  return (
    <div className="zap-chip">
      <ZapEntry zap={zap} />
    </div>
  );
}
