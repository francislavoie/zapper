// About, from the palette: the same pane Settings > About shows, in a dialog
// of its own. It widens for the licenses view, whose list and text need the
// room the summary doesn't.

import { useState } from "react";
import { useDispatch } from "../store";
import { AboutPane } from "./AboutPane";

export function AboutModal() {
  const dispatch = useDispatch();
  const [wide, setWide] = useState(false);
  const close = () => dispatch({ type: "about", open: false });

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) close();
      }}
    >
      <div className={`about ${wide ? "wide" : ""}`} role="dialog" aria-label="About zapper">
        <AboutPane onLicenses={setWide} />
        <div className="about-actions">
          <button className="ctl" onClick={close} autoFocus>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
