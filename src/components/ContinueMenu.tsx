// The context menu over a Continue watching entry: play it (from where it was
// left, or over), its details, mark it watched, or take it off the list.
//
// The surface, its placement and how it closes are ContextMenu's; this is
// only what it says. Drawn once for the whole window, so the list on either
// tab (and anything else that shows the entries) opens the same one.

import {
  openMovieSheet,
  openShowSheet,
  playContinue,
  removeFromContinue,
  setEpisodeWatched,
  setMovieWatched,
} from "../actions";
import { continueKey, continuePlace, continueTitle } from "../logic/continueWatching";
import { resumeLabel, resumePoint, type StartAt } from "../logic/resume";
import { episodeCode } from "../logic/shows";
import { useApp, useDispatch } from "../store";
import type { ContinueItemDto } from "../types";
import { ContextMenu } from "./ContextMenu";

export function ContinueMenu() {
  const { continue: cw } = useApp();
  const dispatch = useDispatch();
  const menu = cw.menu;
  const item: ContinueItemDto | undefined =
    menu && cw.items.status === "ready" ? cw.items.data.find((i) => continueKey(i) === menu.key) : undefined;
  if (!menu || !item) return null;

  const close = () => dispatch({ type: "continue/menu", menu: null });
  const then = (act: () => unknown) => () => {
    close();
    void act();
  };
  const play = (start: StartAt) => then(() => playContinue(dispatch, item, start));

  const at = resumePoint(continuePlace(item));
  const code = item.kind === "episode" ? episodeCode(item.episode.season, item.episode.episode, item.episode.episodeEnd) : "";

  return (
    <ContextMenu x={menu.x} y={menu.y} heading={continueTitle(item)} onClose={close} takeFocus={menu.takeFocus}>
      {at !== null ? (
        <>
          <button className="tb-menu-item" role="menuitem" onClick={play("resume")}>
            <span className="tb-menu-label">{resumeLabel(at)}</span>
          </button>
          <button className="tb-menu-item" role="menuitem" onClick={play(null)}>
            <span className="tb-menu-label">Start over</span>
          </button>
        </>
      ) : (
        <button className="tb-menu-item" role="menuitem" onClick={play(null)}>
          <span className="tb-menu-label">{code !== "" ? `Play ${code}` : "Play"}</span>
        </button>
      )}
      <button
        className="tb-menu-item"
        role="menuitem"
        onClick={then(() =>
          item.kind === "movie" ? openMovieSheet(dispatch, item.movie) : openShowSheet(dispatch, item.series),
        )}
      >
        <span className="tb-menu-label">Details</span>
      </button>
      <button
        className="tb-menu-item"
        role="menuitem"
        onClick={then(() =>
          item.kind === "movie"
            ? setMovieWatched(dispatch, item.movie, true)
            : setEpisodeWatched(dispatch, item.episode, true),
        )}
      >
        <span className="tb-menu-label">{code !== "" ? `Mark ${code} watched` : "Mark watched"}</span>
      </button>
      <button className="tb-menu-item" role="menuitem" onClick={then(() => removeFromContinue(dispatch, item))}>
        <span className="tb-menu-label">Remove from Continue watching</span>
      </button>
    </ContextMenu>
  );
}
