// The ways out of a sheet (a movie's Details, a show's sheet): ‹ Back at the
// top left, where the way out of a full-window thing is on every device, and
// ✕ at the top right, where a pane's close is. Both close it, as Escape and
// the mouse's back button do (useGlobalKeys).

import { Back } from "./icons";

export function SheetControls({ onClose }: { onClose: () => void }) {
  return (
    <>
      {/* Its label is on it, so no tip: tips are for what cannot be read. */}
      <button className="ctl sheet-back" aria-label="Back to the list" onClick={onClose}>
        <Back className="i" />
        Back
      </button>
      <button className="ctl icon movie-sheet-close" aria-label="Close" data-tip="Close (Esc)" onClick={onClose}>
        ✕
      </button>
    </>
  );
}
