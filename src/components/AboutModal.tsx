// About, from the palette: the same pane Settings > About shows, in a dialog
// of its own. It widens for the licenses view, whose list and text need the
// room the summary doesn't.

import { useState } from "react";
import { useDispatch } from "../store";
import { AboutPane } from "./AboutPane";
import { isSideButton } from "../logic/navHistory";

export function AboutModal() {
  const dispatch = useDispatch();
  const [wide, setWide] = useState(false);
  const close = () => dispatch({ type: "about", open: false });

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        // Not the mouse's back button: it closes this as a step of its own.
        if (e.target === e.currentTarget && !isSideButton(e)) close();
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
