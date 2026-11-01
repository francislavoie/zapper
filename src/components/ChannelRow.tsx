// One dense row. Everything here is conditional on the data actually being
// there: most channels have no programme, almost none have an archive, and most
// have never been opened so their quality is unknown. None of those are
// errors and none of them get a warning.

import { memo } from "react";
import {
  failedTip,
  failedToLoad,
  hhmm,
  looksBroken,
  qualityBadge,
  qualityDetail,
} from "../logic/format";
import { ROW_HEIGHT } from "../logic/virtual";
import type { ChannelDto } from "../types";
import { Logo } from "./Logo";

export interface ChannelRowProps {
  channel: ChannelDto;
  top: number;
  selected: boolean;
  pending: boolean;
  onPlay: (c: ChannelDto) => void;
  /** Double-click: start it if it is not already going, and take the window. */
  onOpenFull: (c: ChannelDto) => void;
  /** The 12/24-hour setting. Unread: hhmm follows the setting by itself, and
   *  this prop is only how a change of clock redraws a row the memo would
   *  otherwise keep. */
  clock: string;
}

export const ChannelRow = memo(function ChannelRow({
  channel,
  top,
  selected,
  pending,
  onPlay,
  onOpenFull,
}: ChannelRowProps) {
  const style = { top, height: ROW_HEIGHT };

  if (channel.isSeparator) {
    // A provider-injected divider. Never playable, never a search hit.
    return (
      <div className="row sep" style={style} role="separator">
        {channel.cleanName}
      </div>
    );
  }

  const badge = qualityBadge(channel.quality);
  const now = channel.nowPlaying;
  // Read at render, not on a clock: "3h ago" in a tooltip is close enough
  // until the row next renders, and a timer per row would be a strobe.
  const nowSec = Date.now() / 1000;
  // Dimmed, never hidden: a channel that failed last night may well play
  // now, and the only way to find out is to be able to try it.
  const failed = failedToLoad(channel.quality);

  return (
    <button
      className={`row ${selected ? "sel" : ""} ${pending ? "pending" : ""} ${failed ? "failed" : ""}`}
      style={style}
      data-tip={failedTip(channel.quality, nowSec) ?? undefined}
      onClick={() => onPlay(channel)}
      onDoubleClick={() => onOpenFull(channel)}
    >
      <Logo url={channel.logoUrl} />
      <span className="row-num">{channel.numLabel ?? channel.num ?? ""}</span>
      {/* Tips only when cut short. An instant tip on every row the pointer
          crosses would be a strobe; the name is offered where it is needed,
          which is where the ellipsis is. The raw name rather than the clean
          one, because it is the full story — prefix and all. */}
      <span className="row-name" data-tip-overflow={channel.name}>
        {channel.cleanName}
      </span>
      <span className={`row-now ${now ? "" : "none"}`} data-tip-overflow="">
        {now ? `${hhmm(now.start)}  ${now.title}` : "no guide data"}
      </span>
      {channel.hasArchive ? (
        <span className="badge archive" data-tip={`${channel.archiveDays ?? "?"}-day catchup`}>
          ⏱ {channel.archiveDays ?? "?"}d
        </span>
      ) : null}
      {looksBroken(channel.quality) ? (
        <span
          className="badge broken"
          data-tip={`${channel.quality?.failCount} failures against ${channel.quality?.okCount} successes`}
        >
          !
        </span>
      ) : null}
      {/* No badge at all when quality is null: unknown is not bad. Its tip
          is everything measured, and when. */}
      {badge ? (
        <span className="badge" data-tip={qualityDetail(channel.quality, nowSec) || undefined}>
          {badge}
        </span>
      ) : null}
    </button>
  );
});
