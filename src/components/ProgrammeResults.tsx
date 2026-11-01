// The list's Programmes tab: airings whose title matches the search, on now
// first, then upcoming, then what an archive can still play.
//
// Not virtualized like the channel list: results arrive a page at a time and
// a few hundred plain rows is nothing, where tens of thousands of channels
// are not. Each row says when, what, and where; choosing one tunes in, plays
// the archive, or opens the guide on it — see openProgrammeHit.

import { memo } from "react";
import { openProgrammeHit } from "../actions";
import { useClock } from "../hooks/useClock";
import type { ProgrammeSearch } from "../hooks/useProgrammeSearch";
import { airingTag, airingWhen, hitTip } from "../logic/programmes";
import { useApp, useDispatch } from "../store";
import type { ProgrammeHitDto } from "../types";
import { Logo } from "./Logo";

export function ProgrammeResults({ search, query }: { search: ProgrammeSearch; query: string }) {
  const state = useApp();
  const dispatch = useDispatch();
  const now = useClock(15_000);
  const { result } = search;
  const hits = result.status === "ready" ? result.data : [];

  const choose = (hit: ProgrammeHitDto) =>
    void openProgrammeHit(dispatch, hit, {
      categoryId: state.categoryId,
      mode: state.mode,
      guideHours: state.settings.guideHours,
    });

  return (
    <div className="viewport hits">
      {hits.map((hit) => (
        <HitRow
          key={`${hit.channel.streamId}:${hit.programme.start}`}
          hit={hit}
          now={now}
          selected={hit.airing === "live" && state.player.streamId === hit.channel.streamId}
          onChoose={choose}
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
  now,
  selected,
  onChoose,
}: {
  hit: ProgrammeHitDto;
  now: number;
  selected: boolean;
  onChoose: (hit: ProgrammeHitDto) => void;
}) {
  const { channel, programme, airing } = hit;
  return (
    <button className={`hit ${airing} ${selected ? "sel" : ""}`} onClick={() => onChoose(hit)}>
      <span className={`hit-tag ${airing}`} data-tip={hitTip(airing)}>
        {airingTag(airing)}
      </span>
      <span className="hit-when">{airingWhen(hit, now)}</span>
      <span className="hit-title" data-tip-overflow="">
        {programme.title}
      </span>
      <Logo url={channel.logoUrl} />
      <span className="hit-chan" data-tip-overflow={channel.name}>
        <span className="hit-num">{channel.numLabel ?? channel.num ?? ""}</span>
        {channel.cleanName}
      </span>
    </button>
  );
});
