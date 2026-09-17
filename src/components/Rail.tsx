// The left rail, and the tab bar above it.
//
// Two lists live here and only one is on screen: the categories, which are
// the catalogue's own structure, and the favourites, which are this
// installation's. They share the column because they answer the same question
// — what do I want to watch — from opposite ends, and because the rail is the
// only place in the layout where a second list costs nothing.

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

  return (
    <aside className="rail">
      <div className="rail-tabs" role="tablist" aria-label="Channel lists">
        {TABS.map((t) => (
          <button
            key={t.id}
            className={`rail-tab ${railTab === t.id ? "sel" : ""}`}
            role="tab"
            aria-selected={railTab === t.id}
            onClick={() => dispatch({ type: "railTab", tab: t.id })}
          >
            {t.label}
          </button>
        ))}
      </div>

      {railTab === "categories" ? <CategoryRail /> : <FavouritesRail />}
    </aside>
  );
}
