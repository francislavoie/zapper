// Hundreds of categories are far too many to scan, so the rail is filterable
// and the filter is local: the backend's nucleo index is for channels, and
// spending a round trip on a few-hundred-item substring match would be silly.

import { useEffect, useMemo, useRef } from "react";
import { backend } from "../backend";
import { errorMessage, failed, loading, ready } from "../logic/async";
import { ALL_CATEGORIES, useApp, useDispatch, type Action } from "../store";
import type { Dispatch } from "react";

export function CategoryRail() {
  const { categories, categoryId, categoryFilter, sync } = useApp();
  const dispatch = useDispatch();
  const requested = useRef(false);

  useEffect(() => {
    if (requested.current) return;
    requested.current = true;
    void load(dispatch);
  }, [dispatch]);

  const list = useMemo(() => {
    if (categories.status !== "ready") return [];
    const q = categoryFilter.trim().toLowerCase();
    if (q === "") return categories.data;
    return categories.data.filter((c) => c.name.toLowerCase().includes(q));
  }, [categories, categoryFilter]);

  const total =
    categories.status === "ready"
      ? categories.data.reduce((n, c) => n + c.channelCount, 0)
      : sync.channelsTotal;

  return (
    <aside className="rail">
      <div className="pane-head">
        <input
          className="field"
          placeholder="Filter categories"
          value={categoryFilter}
          spellCheck={false}
          onChange={(e) => dispatch({ type: "categoryFilter", text: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === "Escape" && categoryFilter !== "") {
              e.stopPropagation();
              dispatch({ type: "categoryFilter", text: "" });
            }
          }}
        />
        <button
          className="ctl ghost"
          title="Settings"
          onClick={() => dispatch({ type: "settings", open: true })}
        >
          ⚙
        </button>
      </div>

      <div className="scroller">
        <button
          className={`rail-item ${categoryId === ALL_CATEGORIES ? "sel" : ""}`}
          onClick={() => dispatch({ type: "selectCategory", id: ALL_CATEGORIES })}
        >
          <span className="rail-name">All channels</span>
          <span className="rail-count">{total ? total.toLocaleString() : ""}</span>
        </button>

        {categories.status === "loading" ? (
          <div className="pane-note">Loading categories…</div>
        ) : null}

        {categories.status === "error" ? (
          <div className="pane-error">
            <span>Categories unavailable — {categories.message}</span>
            <button className="link" onClick={() => void load(dispatch)}>
              try again
            </button>
          </div>
        ) : null}

        {categories.status === "ready" && list.length === 0 ? (
          <div className="pane-note">
            {categories.data.length === 0
              ? "No categories yet. Sync has not run."
              : `No category matches “${categoryFilter}”.`}
          </div>
        ) : null}

        {list.map((c) => (
          <button
            key={c.id}
            className={`rail-item ${c.id === categoryId ? "sel" : ""}`}
            title={c.name}
            onClick={() => dispatch({ type: "selectCategory", id: c.id })}
          >
            <span className="rail-name">{c.name}</span>
            <span className="rail-count">{c.channelCount.toLocaleString()}</span>
          </button>
        ))}
      </div>
    </aside>
  );
}

async function load(dispatch: Dispatch<Action>) {
  dispatch({ type: "categories", value: loading() });
  try {
    dispatch({ type: "categories", value: ready(await backend.categories()) });
  } catch (e) {
    dispatch({ type: "categories", value: failed(errorMessage(e)) });
  }
}
