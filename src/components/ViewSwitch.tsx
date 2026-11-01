// The way between the channel list and the guide, drawn where you are.
//
// The guide was reachable only by G, the ☰ menu and the command palette —
// none of them visible from the list, and G types a "g" while the search box
// has focus, which is exactly where you are while looking for a channel. So
// both views carry the same two-way switch in their header, in the same
// place, and the shortcut is in its tooltip for whoever wants it.

import { useDispatch } from "../store";

export function ViewSwitch({ current }: { current: "browse" | "guide" }) {
  const dispatch = useDispatch();
  const views: Array<["browse" | "guide", string, string]> = [
    ["browse", "List", "The channel list (Escape from the guide)"],
    ["guide", "Guide", "What's on, as a timeline (G)"],
  ];
  return (
    <div className="view-switch" role="tablist" aria-label="View">
      {views.map(([mode, label, title]) => (
        <button
          key={mode}
          role="tab"
          aria-selected={current === mode}
          className={current === mode ? "on" : ""}
          title={title}
          onClick={() => {
            if (current !== mode) dispatch({ type: "mode", mode });
          }}
        >
          {label}
        </button>
      ))}
    </div>
  );
}
