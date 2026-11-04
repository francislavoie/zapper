// The list's Programmes tab: airings whose title matches the search, on now
// first, then upcoming, then what an archive can still play.
//
// Not virtualized like the channel list: results arrive a page at a time and
// a few hundred plain rows is nothing, where tens of thousands of channels
// are not. Each row says when, what, and where — the channel and its
// category, since the same channel name turns up in many; choosing one tunes
// in, plays the archive, or opens the guide on it — see openProgrammeHit.

import { memo, useCallback, useMemo } from "react";
import { openProgrammeHit } from "../actions";
import { useClock } from "../hooks/useClock";
import type { ProgrammeSearch } from "../hooks/useProgrammeSearch";
import { airingTag, airingWhen, hitTip } from "../logic/programmes";
import { categoryNames, categoryOf } from "../logic/searchGroups";
import { listedFor } from "../logic/railItems";
import { useApp, useDispatch } from "../store";
import { markKey, marks } from "../logic/recordings";
import { programmeMenuProps, type ProgrammeMenuOpener } from "../programmeMenu";
import type { ProgrammeHitDto, RecordingDto } from "../types";
import { Logo } from "./Logo";
import { RecDot } from "./RecDot";

export function ProgrammeResults({ search, query }: { search: ProgrammeSearch; query: string }) {
  const state = useApp();
  const dispatch = useDispatch();
  const now = useClock(15_000);
  const { result } = search;
  const hits = result.status === "ready" ? result.data : [];
  const names = useMemo(
    () => categoryNames(state.categories.status === "ready" ? state.categories.data : []),
    [state.categories],
  );

  const choose = (hit: ProgrammeHitDto) =>
    void openProgrammeHit(dispatch, hit, {
      categoryId: state.categoryId,
      mode: state.mode,
      guideHours: state.settings.guideHours,
      listed: listedFor(
        state.categoryId,
        state.favourites.status === "ready" ? state.favourites.data : null,
        state.recent.status === "ready" ? state.recent.data : null,
      ),
    });

  const recorded = useMemo(() => marks(state.recordings ?? []), [state.recordings]);
  const onMenu = useCallback<ProgrammeMenuOpener>(
    (channel, programme, at, keyboard) =>
      dispatch({ type: "programmeMenu", menu: { channel, programme, ...at, keyboard } }),
    [dispatch],
  );

  return (
    <div className="viewport hits">
      {hits.map((hit) => (
        <HitRow
          key={`${hit.channel.streamId}:${hit.programme.start}`}
          hit={hit}
          category={categoryOf(names, hit.channel.categoryId)}
          now={now}
          clock={state.settings.timeFormat}
          selected={hit.airing === "live" && state.player.streamId === hit.channel.streamId}
          onChoose={choose}
          onMenu={onMenu}
          recording={recorded.get(markKey(hit.channel.streamId, hit.programme.start))}
        />
      ))}
      {search.hasMore ? (
        <button className="hits-more link" disabled={search.loadingMore} onClick={search.more}>
          {search.loadingMore ? "Loading…" : "More results"}
        </button>
      ) : null}
      {result.status === "error" ? (
        <div className="pane-note">Programme search failed — {result.message}</div>
      ) : result.status === "ready" && hits.length === 0 ? (
        <div className="pane-note">
          Nothing in the guide matches “{query}”, on now, coming up, or in a catch-up archive.
        </div>
      ) : null}
    </div>
  );
}

const HitRow = memo(function HitRow({
  hit,
  category,
  now,
  selected,
  onChoose,
  onMenu,
  recording,
}: {
  hit: ProgrammeHitDto;
  category: string;
  now: number;
  selected: boolean;
  onChoose: (hit: ProgrammeHitDto) => void;
  /** The 12/24-hour setting. Unread: airingWhen's times follow the setting
   *  by themselves, and this prop is only how a change of clock redraws a
   *  row the memo would otherwise keep. */
  clock: string;
  onMenu: ProgrammeMenuOpener;
  recording: RecordingDto | undefined;
}) {
  const { channel, programme, airing } = hit;
  return (
    <button
      className={`hit ${airing} ${selected ? "sel" : ""}`}
      onClick={() => onChoose(hit)}
      {...programmeMenuProps(channel, programme, onMenu)}
    >
      <span className={`hit-tag ${airing}`} data-tip={hitTip(airing)}>
        {airingTag(airing)}
      </span>
      <span className="hit-when">{airingWhen(hit, now)}</span>
      <span className="hit-title" data-tip-overflow="">
        {recording ? <RecDot recording={recording} /> : null}
        {programme.title}
      </span>
      <Logo url={channel.logoUrl} />
      <span className="hit-chan" data-tip-overflow={channel.name}>
        <span className="hit-num">{channel.numLabel ?? channel.num ?? ""}</span>
        {channel.cleanName}
      </span>
      <span className="hit-cat" data-tip-overflow="">
        {category}
      </span>
    </button>
  );
});
