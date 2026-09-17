// The favourites half of the rail.
//
// Nothing here adds a favourite, because nothing can: the list is accumulated
// by the player from what is actually watched, ranked by watch time decayed
// against how long ago it was, and handed over by the backend already in
// order. This pane's whole job is to show it, play from it, and forget a row
// that should not be there.
//
// An empty list is the ordinary first-run state — the table starts empty and
// fills itself — so it says so plainly instead of showing a blank pane or an
// error.

import { useCallback, useEffect, useState } from "react";
import { playChannel } from "../actions";
import { backend } from "../backend";
import { useClock } from "../hooks/useClock";
import { errorMessage, failed, loading, ready, type Async } from "../logic/async";
import { favouriteMeta } from "../logic/favourites";
import { run, useApp, useDispatch } from "../store";
import type { FavouriteDto } from "../types";
import { FavouriteMenu } from "./FavouriteMenu";

/** More than fills the rail at any zoom, and short enough that the backend's
 *  join and the ranking behind it stay trivial. */
export const FAVOURITE_LIMIT = 40;

export function FavouritesRail() {
  const { player, pendingStreamId, favouriteMenu } = useApp();
  const dispatch = useDispatch();
  const [list, setList] = useState<Async<FavouriteDto[]>>(loading());
  // "3 days ago" has to stop being true eventually; a coarse tick is plenty.
  const now = useClock(60_000);

  const load = useCallback(async () => {
    setList(loading());
    try {
      setList(ready(await backend.favourites(FAVOURITE_LIMIT)));
    } catch (e) {
      setList(failed(errorMessage(e)));
    }
  }, []);

  // Reloaded when the channel changes as well as on mount: watching something
  // is the only thing that can change this list, and a list that only updated
  // on a restart would look broken in exactly the way the feature is about.
  useEffect(() => {
    void load();
  }, [load, player.streamId]);

  const forget = async (streamId: number) => {
    dispatch({ type: "favouriteMenu", menu: null });
    const ok = await run(dispatch, "forget", backend.forgetActivity(streamId));
    if (ok) await load();
  };

  return (
    <div className="scroller fav-list">
      {list.status === "loading" ? <div className="pane-note">Loading favourites…</div> : null}

      {list.status === "error" ? (
        <div className="pane-error">
          <span>Favourites unavailable — {list.message}</span>
          <button className="link" onClick={() => void load()}>
            try again
          </button>
        </div>
      ) : null}

      {list.status === "ready" && list.data.length === 0 ? (
        // Not an error and not a blank pane: this is what the first run looks
        // like, and there is nothing for anyone to do about it.
        <div className="pane-note">
          Nothing here yet. Favourites collect themselves from what you watch —
          a channel joins the list after about half a minute on it.
        </div>
      ) : null}

      {list.status === "ready"
        ? list.data.map((f) => (
            <button
              key={f.channel.streamId}
              className={`fav-item ${player.streamId === f.channel.streamId ? "sel" : ""} ${
                pendingStreamId === f.channel.streamId ? "pending" : ""
              }`}
              title={f.channel.name}
              onClick={() => void playChannel(dispatch, f.channel)}
              onContextMenu={(e) => {
                // WebKit's own menu is suppressed at the GTK level, so this
                // event arrives here in the real app exactly as it does in a
                // browser; preventDefault is for the browser-driven mock.
                e.preventDefault();
                dispatch({
                  type: "favouriteMenu",
                  menu: {
                    streamId: f.channel.streamId,
                    name: f.channel.cleanName,
                    x: e.clientX,
                    y: e.clientY,
                  },
                });
              }}
            >
              <span className="fav-name">{f.channel.cleanName}</span>
              <span className="fav-meta">
                {favouriteMeta(f.watchSeconds, f.lastWatched, now)}
              </span>
            </button>
          ))
        : null}

      {favouriteMenu ? (
        <FavouriteMenu
          menu={favouriteMenu}
          onForget={() => void forget(favouriteMenu.streamId)}
          onClose={() => dispatch({ type: "favouriteMenu", menu: null })}
        />
      ) : null}
    </div>
  );
}
