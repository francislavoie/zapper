// The left rail, and the tab bar above it.
//
// Two lists live here and only one is on screen: the categories, which are
// the catalogue's own structure, and the favourites, which are this
// installation's. They share the column because they answer the same question
// — what do I want to watch — from opposite ends, and because the rail is the
// only place in the layout where a second list costs nothing.
//
// Both are always mounted, inside <Activity>. Unmounting the pane being left
// would throw away everything it had: the categories would be re-fetched and
// the rail would say "Loading categories" every single time the tab was
// clicked, and the scroll position, the filter text and the favourites list
// would go with it. Activity keeps a hidden tree's state and its DOM while
// unmounting its effects, so coming back is instant and nothing re-fetches
// that does not need to.
//
// The switch itself is a transition. It is not urgent — the tab highlights
// immediately either way — and marking it keeps a long list rendering from
// blocking the click.

import { Activity, useTransition } from "react";
import { CategoryRail } from "./CategoryRail";
import { FavouritesRail } from "./FavouritesRail";
import { useApp, useDispatch, type RailTab } from "../store";

const TABS: Array<{ id: RailTab; label: string }> = [
  { id: "categories", label: "Categories" },
  { id: "favourites", label: "Favourites" },
];

export function Rail() {
  const { railTab } = useApp();
  const dispatch = useDispatch();
  const [switching, startSwitch] = useTransition();

  return (
    <aside className={`rail ${switching ? "switching" : ""}`}>
      <div className="rail-tabs" role="tablist" aria-label="Channel lists">
        {TABS.map((t) => (
          <button
            key={t.id}
            className={`rail-tab ${railTab === t.id ? "sel" : ""}`}
            role="tab"
            aria-selected={railTab === t.id}
            onClick={() => startSwitch(() => dispatch({ type: "railTab", tab: t.id }))}
          >
            {t.label}
          </button>
        ))}
      </div>

      {/* `mode` is the whole switch. A hidden Activity renders nothing and
          costs nothing to keep, and its subtree comes back exactly as it was
          left. */}
      <Activity mode={railTab === "categories" ? "visible" : "hidden"}>
        <CategoryRail />
      </Activity>
      <Activity mode={railTab === "favourites" ? "visible" : "hidden"}>
        <FavouritesRail />
      </Activity>
    </aside>
  );
}
