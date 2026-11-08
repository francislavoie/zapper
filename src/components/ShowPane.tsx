// The Shows tab's episode pane: beside the list (master–detail), on the show
// selected there. A click on a show opens it; the arrow keys in the list move
// the selection and the pane follows; Enter or → go into its episodes, Enter
// there plays, `[` `]` change season, ← or Escape go back to the list, and a
// second Escape (or ✕, or the mouse's back button) closes it
// (logic/showPane.ts, logic/escape.ts).
//
// Its own scroller: revealing what is next moves the episode list and
// nothing else, neither the list beside it nor the band above.
//
// Stacked under the picture there is no room beside the list, so it opens in
// the list's place: "‹ <the rail's choice>" to go back, and the show's
// poster, facts and actions standing in for the band there is not.

import { useEffect, useId, useMemo, useRef, type KeyboardEvent } from "react";
import { playEpisode, setEpisodeWatched } from "../actions";
import { useShowDetailsOf } from "../hooks/useShowDetails";
import type { Async } from "../logic/async";
import { clock, positionLabel } from "../logic/format";
import { DETAILS_DELAY_MS, playingKind } from "../logic/movies";
import {
  episodeCode,
  episodeProgress,
  episodeTitle,
  firstSeason,
  seasonLabel,
  seasonMeta,
  seasonsText,
  showAction,
} from "../logic/shows";
import { paneKey, stepSeason } from "../logic/showPane";
import { useApp, useDispatch } from "../store";
import type { EpisodeDto, SeriesDetailsDto, SeriesDto } from "../types";
import { BandPoster } from "./BandParts";
import { EpisodeList } from "./EpisodeList";
import { ShowActions, ShowFacts, ShowPlayButton } from "./ShowBand";
import { Back, Fullscreen } from "./icons";

export interface ShowPaneProps {
  show: SeriesDto;
  /** In the list's place, under the picture, rather than beside it. */
  stacked: boolean;
  /** What the list shows ("Recently added"), for the stacked pane's way back. */
  listName: string;
  /** The list has asked for focus to come into this show's episodes
   *  (Enter, →): it does once they are here, and `settled` says the ask is
   *  answered — or cannot be, the episodes having failed to load. */
  wantsFocus: boolean;
  settled: () => void;
  /** Focus back to the list beside it (←, Escape from the episodes). */
  toList: () => void;
}

export function ShowPane({ show, stacked, listName, wantsFocus, settled, toList }: ShowPaneProps) {
  const { shows, player } = useApp();
  const dispatch = useDispatch();
  const root = useRef<HTMLElement | null>(null);
  // As the band's: the arrow keys cross shows without asking about each.
  const details = useShowDetailsOf(show, DETAILS_DELAY_MS);
  const d = details.status === "ready" ? details.data : null;

  const season =
    shows.season !== null && d?.seasons.some((s) => s.number === shows.season)
      ? shows.season
      : d
        ? firstSeason(d)
        : null;
  const episodes = useMemo(
    () => (d && season !== null ? d.episodes.filter((e) => e.season === season) : []),
    [d, season],
  );
  const seasons = d?.seasons ?? [];
  const chosen = seasons.find((s) => s.number === season) ?? null;
  const playingEpisode =
    playingKind(player) === "episode" && shows.playing?.series.seriesId === show.seriesId
      ? shows.playing.episode
      : null;

  const focusEpisodes = () =>
    requestAnimationFrame(() => root.current?.querySelector<HTMLElement>(".ep-list")?.focus());

  // Into the episodes once asked (Enter or → in the list), as soon as they
  // are here: the cursor is on what is next.
  const listed = d !== null && season !== null;
  const failed = details.status === "error";
  useEffect(() => {
    if (!wantsFocus) return;
    if (failed) settled();
    else if (listed) {
      settled();
      focusEpisodes();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wantsFocus, listed, failed]);

  // Stacked, the pane takes the place of the list that had focus: focus
  // comes to its way back rather than falling to the page (or to the
  // episodes, above, when Enter or → opened it).
  useEffect(() => {
    if (stacked && !wantsFocus) root.current?.querySelector<HTMLElement>(".sp-back")?.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Closed with focus inside it (✕, Escape from its season tabs): back to
  // the list rather than to nowhere.
  const toListRef = useRef(toList);
  toListRef.current = toList;
  useEffect(
    () => () => {
      const here = document.activeElement;
      if (!here || here === document.body) toListRef.current();
    },
    [],
  );

  const choose = (n: number) => dispatch({ type: "shows/season", number: n });
  const close = () => dispatch({ type: "shows/pane", open: false });
  // Back to the list: beside it, the pane stays open; stacked, it is in the
  // list's place and goes.
  const back = () => {
    if (stacked) close();
    toList();
  };

  // The pane's keys in the episodes (logic/showPane.ts); the list's own
  // (↑ ↓, Enter) are left to it, and the rest of the pane's (→) taken so
  // that they do not reach the player.
  const onKeys = (key: string): boolean => {
    const move = paneKey("episodes", key, { selected: true, posters: false });
    if (!move || move.play) return false;
    if (move.zone === "list") {
      back();
      return true;
    }
    if (move.season) {
      const to = stepSeason(
        seasons.map((s) => s.number),
        season,
        move.season,
      );
      if (to !== null && to !== season) {
        choose(to);
        // The list is keyed by the season: the new one takes the focus.
        focusEpisodes();
      }
      return true;
    }
    return true;
  };

  const play = (episode: EpisodeDto) => void playEpisode(dispatch, { series: show, episode });
  const mark = (episode: EpisodeDto, watched: boolean) => void setEpisodeWatched(dispatch, episode, watched);
  const openSheet = () => dispatch({ type: "shows/sheet", id: show.seriesId });

  const seasonCount = d ? d.seasons.filter((s) => s.number !== 0).length : show.seasonCount;
  const ids = useId();
  const tabId = (n: number) => `${ids}-season-${n}`;
  const panelId = `${ids}-episodes`;
  const main = <PaneMainAction show={show} details={details} playingEpisode={playingEpisode} />;

  return (
    <section
      className={`show-pane ${stacked ? "stacked" : ""}`}
      aria-label={`${show.cleanName}: episodes`}
      ref={root}
    >
      {stacked ? (
        <>
          <header className="sp-head drill">
            <button className="ctl sp-back" onClick={close}>
              <Back className="i" />
              <span className="sp-back-name" data-tip-overflow="">
                {listName}
              </span>
            </button>
            <span className="spring" />
            <button className="ctl ghost" onClick={openSheet}>
              Details ›
            </button>
          </header>
          <div className="sp-drill">
            <BandPoster url={show.posterUrl} name={show.cleanName} />
            <div className="band-text">
              <div className="watch-body">
                <h2 className="watch-title-h" data-tip-overflow={show.name}>
                  {show.cleanName}
                </h2>
                <ShowFacts show={show} details={details} className="show-band-facts" />
                {d?.plot ? (
                  <p className="watch-desc" data-tip-overflow="">
                    {d.plot}
                  </p>
                ) : null}
              </div>
              <div className="watch-actions">
                <ShowActions show={show} details={details} playing={false} main={main} />
              </div>
            </div>
          </div>
        </>
      ) : (
        <>
          <header className="sp-head">
            <h3 className="sp-title" data-tip-overflow={show.name}>
              {show.cleanName}
            </h3>
            {seasonCount ? <span className="sp-facts">{seasonsText(seasonCount)}</span> : null}
            <span className="spring" />
            <button className="ctl ghost" onClick={openSheet}>
              Details ›
            </button>
            <button className="ctl icon" aria-label="Close" data-tip="Close (Esc)" onClick={close}>
              ✕
            </button>
          </header>
          <div className="sp-act">{main}</div>
        </>
      )}

      {seasons.length > 0 ? (
        <SeasonTabs
          seasons={seasons}
          season={season}
          meta={chosen ? seasonMeta(chosen) : ""}
          tabId={tabId}
          panelId={panelId}
          onChoose={choose}
          onBack={back}
        />
      ) : null}

      <div className="sp-eps">
        {details.status === "loading" || details.status === "idle" ? (
          <p className="sp-note">Loading episodes…</p>
        ) : details.status === "error" ? (
          <p className="sp-note">Episodes unavailable</p>
        ) : d && seasons.length === 0 ? (
          <p className="sp-note">No episodes yet</p>
        ) : null}
        {d && season !== null ? (
          <div role="tabpanel" id={panelId} aria-labelledby={tabId(season)}>
            <EpisodeList
              key={`${show.seriesId}:${season}`}
              episodes={episodes}
              nextUp={d.nextUp}
              playingId={playingEpisode?.episodeId ?? null}
              onPlay={play}
              onWatched={mark}
              onKeys={onKeys}
            />
          </div>
        ) : null}
      </div>

      <footer className="sp-keys" aria-hidden>
        <span>
          <b>↑↓</b> episodes
        </span>
        <span>
          <b>Enter</b> play
        </span>
        {seasons.length > 1 ? (
          <span className="opt">
            <b>[ ]</b> season
          </span>
        ) : null}
        {stacked ? (
          <span>
            <b>Esc</b> or <b>←</b> back to the list
          </span>
        ) : (
          <>
            <span>
              <b>←</b> shows
            </span>
            <span>
              <b>Esc</b> close
            </span>
          </>
        )}
      </footer>
    </section>
  );
}

/** The pane's main action, beside the list and stacked alike: Play (what is
 *  next, with where it was left), or for the show playing, back to it full
 *  screen. The one place it is built. */
function PaneMainAction({
  show,
  details,
  playingEpisode,
}: {
  show: SeriesDto;
  details: Async<SeriesDetailsDto>;
  playingEpisode: EpisodeDto | null;
}) {
  const d = details.status === "ready" ? details.data : null;
  const { player } = useApp();
  const dispatch = useDispatch();
  if (playingEpisode) {
    const at = player.onDemand?.kind === "episode" ? player.onDemand : null;
    return (
      <>
        <button className="ctl movie-play" onClick={() => dispatch({ type: "mode", mode: "watch" })}>
          <Fullscreen className="i" /> Back to {codeOf(playingEpisode)}
        </button>
        {at ? <span className="sp-hint">playing · {positionLabel(at).replace(" / ", " of ")}</span> : null}
      </>
    );
  }
  const action = d ? showAction(d) : null;
  const e = action && d ? d.episodes.find((x) => x.episodeId === action.episodeId) : undefined;
  const hint = e
    ? episodeProgress(e) !== null && e.position !== null
      ? `from ${clock(e.position)} · ${episodeTitle(e)}`
      : episodeTitle(e)
    : "";
  return (
    <>
      <ShowPlayButton show={show} details={details} />
      {hint ? (
        <span className="sp-hint" data-tip-overflow="">
          {hint}
        </span>
      ) : null}
    </>
  );
}

const codeOf = (e: EpisodeDto) => episodeCode(e.season, e.episode, e.episodeEnd);

/** The seasons as a row of tabs, the chosen one's counts at its end. */
function SeasonTabs({
  seasons,
  season,
  meta,
  tabId,
  panelId,
  onChoose,
  onBack,
}: {
  seasons: SeriesDetailsDto["seasons"];
  season: number | null;
  meta: string;
  tabId: (n: number) => string;
  panelId: string;
  onChoose: (n: number) => void;
  /** Escape: back to the list. */
  onBack: () => void;
}) {
  const tabs = useRef<HTMLDivElement | null>(null);
  // The chosen tab in sight in its row, which scrolls sideways when a show
  // has more seasons than fit.
  useEffect(() => {
    const row = tabs.current;
    const tab = row?.querySelector<HTMLElement>('[aria-selected="true"]');
    if (!row || !tab) return;
    const left = tab.offsetLeft - row.offsetLeft;
    const right = left + tab.offsetWidth;
    if (left < row.scrollLeft) row.scrollLeft = left;
    else if (right > row.scrollLeft + row.clientWidth) row.scrollLeft = right - row.clientWidth;
  }, [season]);

  // ← → between the seasons, Home and End to either end: focus follows the
  // choice, as a tab row's does.
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      // Not the Escape chain, which would close the pane.
      e.preventDefault();
      e.stopPropagation();
      onBack();
      return;
    }
    const at = seasons.findIndex((s) => s.number === season);
    let to: number;
    switch (e.key) {
      case "ArrowLeft":
        to = at - 1;
        break;
      case "ArrowRight":
        to = at + 1;
        break;
      case "Home":
        to = 0;
        break;
      case "End":
        to = seasons.length - 1;
        break;
      default:
        return;
    }
    // Not the player's seek as well.
    e.preventDefault();
    e.stopPropagation();
    const next = seasons[Math.max(0, Math.min(seasons.length - 1, to))];
    if (!next) return;
    onChoose(next.number);
    requestAnimationFrame(() =>
      tabs.current?.querySelector<HTMLElement>(`[data-season="${next.number}"]`)?.focus(),
    );
  };

  return (
    <div className="seasons">
      <div className="seasons-tabs" role="tablist" aria-label="Seasons" ref={tabs} onKeyDown={onKeyDown}>
        {seasons.map((s) => (
          <button
            key={s.number}
            id={tabId(s.number)}
            aria-controls={s.number === season ? panelId : undefined}
            className={`seasons-tab ${s.number === season ? "on" : ""}`}
            role="tab"
            aria-selected={s.number === season}
            tabIndex={s.number === season ? 0 : -1}
            data-season={s.number}
            data-tip-overflow={s.name}
            onClick={() => onChoose(s.number)}
          >
            {s.name || seasonLabel(s.number)}
          </button>
        ))}
      </div>
      {meta ? <span className="season-meta">{meta}</span> : null}
    </div>
  );
}
