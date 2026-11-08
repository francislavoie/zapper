// Up next: near an episode's end, the episode after it, a count down to
// playing it, and the two things to do about that — play it now, or let
// this one play to its end without moving on. In the full player (with the
// next one's still) and in the docked picture (compact: none). The count
// is useUpNext's, which plays the next one when it gets to nothing; the
// buttons are in the tab order, and in the full player Enter and Escape
// answer for them (useGlobalKeys).
//
// Over the picture, in a panel of its own, which stays while the player's
// controls fade: the countdown is the one thing worth seeing then.

import { playNextEpisode } from "../actions";
import { runtime } from "../logic/movies";
import { episodeCode, episodeTitle } from "../logic/shows";
import { UP_NEXT_COUNTDOWN_SECS, type UpNext } from "../logic/upNext";
import { useApp, useDispatch } from "../store";
import { Poster } from "./Poster";

/** The ring's circumference, in its own viewBox units (radius 8). */
const RING = 2 * Math.PI * 8;

export function UpNextCard({ upNext, compact = false }: { upNext: UpNext; compact?: boolean }) {
  const { shows } = useApp();
  const dispatch = useDispatch();
  const from = shows.playing;
  const next = upNext.kind === "none" || from === null ? null : upNext.next;
  const title = next ? episodeTitle(next) : "";
  const line = next ? `${episodeCode(next.season, next.episode, next.episodeEnd)} · ${title}` : "";
  // Said once, as the card appears, by a region that is there before it
  // (one added with its words is not reliably read out); the count that
  // changes every second is not.
  const announce = (
    <span className="upnext-announce" role="status">
      {next ? `Up next: ${line}` : ""}
    </span>
  );
  if (next === null || from === null) return announce;
  const left = upNext.kind === "counting" ? upNext.secondsLeft : 0;
  const length = runtime(next.durationSecs ?? next.duration);
  const done = 1 - Math.min(1, Math.max(0, left / UP_NEXT_COUNTDOWN_SECS));

  return (
    <>
      {announce}
      <div
        className={`upnext ${compact ? "compact" : ""}`}
        role="region"
        aria-label="Up next"
        // Its clicks are its own: not a pause, nor the docked picture's way
        // into the player.
        onClick={(e) => e.stopPropagation()}
        onDoubleClick={(e) => e.stopPropagation()}
        onContextMenu={(e) => e.stopPropagation()}
      >
        {compact ? null : (
          <span className="upnext-still">
            <Poster url={next.stillUrl} name={title} className="upnext-still-img" />
          </span>
        )}
        <div className="upnext-body">
          <span className="upnext-label">Up next</span>
          <h5 className="upnext-title" data-tip-overflow={line}>
            {line}
          </h5>
          {compact || !length ? null : <p className="upnext-meta">{length}</p>}
          <div className="upnext-count">
            <svg className="upnext-ring" viewBox="0 0 20 20" aria-hidden>
              <circle className="upnext-ring-track" cx="10" cy="10" r="8" />
              <circle
                className="upnext-ring-done"
                cx="10"
                cy="10"
                r="8"
                strokeDasharray={RING}
                strokeDashoffset={RING * (1 - done)}
              />
            </svg>
            <span>Playing in {left}</span>
          </div>
          <div className="upnext-btns">
            <button
              className="ctl upnext-play"
              data-tip={compact ? "Play the next episode now" : "Play the next episode now (Enter)"}
              onClick={() => void playNextEpisode(dispatch, from, next)}
            >
              ▶ Play now
            </button>
            <button
              className="ctl upnext-cancel"
              data-tip={compact ? "Cancel Up next" : "Cancel Up next (Escape)"}
              onClick={() => dispatch({ type: "shows/upNextCancel", episodeId: from.episode.episodeId })}
            >
              Cancel
            </button>
          </div>
        </div>
      </div>
    </>
  );
}
