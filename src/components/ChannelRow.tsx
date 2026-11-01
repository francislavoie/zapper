// One dense row. Everything here is conditional on the data actually being
// there: most channels have no programme, almost none have an archive, and most
// have never been opened so their quality is unknown. None of those are
// errors and none of them get a warning.

import { memo } from "react";
import { hhmm, looksBroken, qualityBadge } from "../logic/format";
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

  return (
    <button
      className={`row ${selected ? "sel" : ""} ${pending ? "pending" : ""}`}
      style={style}
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
      {/* No badge at all when quality is null: unknown is not bad. */}
      {badge ? <span className="badge">{badge}</span> : null}
    </button>
  );
});
