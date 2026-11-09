// Whether the window is maximised, which the maximise button's icon has to
// reflect.
//
// Asked rather than remembered: the window can be maximised by a keyboard
// shortcut, by the window manager, or by double-clicking our own drag handle,
// and a local boolean would be wrong after any of those. Every one of them
// changes the window's size, so the DOM resize event is the trigger to ask
// again — no extra window event needed for what is already observable from
// the page.

import { useEffect, useState } from "react";
import { windowControls } from "../window";

export function useMaximised(): boolean {
  const [maximised, setMaximised] = useState(false);

  useEffect(() => {
    let live = true;
    const ask = () => {
      void windowControls
        .isMaximised()
        .then((v) => live && setMaximised(v))
        .catch(() => {
          // A window that will not answer keeps the icon it has. Nothing here
          // is worth a toast: the button still works either way.
        });
    };
    ask();
    window.addEventListener("resize", ask);
    return () => {
      live = false;
      window.removeEventListener("resize", ask);
    };
  }, []);

  return maximised;
}
