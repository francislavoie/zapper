// One two-line row: the name over its category, what is on with how far
// through it is, and what is next. Everything here is conditional on the data actually being
// there: most channels have no programme, almost none have an archive, and most
// have never been opened so their quality is unknown. None of those are
// errors and none of them get a warning.

import { memo } from "react";
import {
  failedTip,
  failedToLoad,
  looksBroken,
  programmeProgress,
  qualityBadge,
  qualityDetail,
  timeRange,
} from "../logic/format";
import { LIST_ROW_HEIGHT } from "../logic/virtual";
import type { ChannelDto, ProgrammeDto } from "../types";
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
  /** Where the channel lives, as the dim second line under its name. Null
   *  when the category is not known. */
  categoryName: string | null;
  /** The programme on air: the guide's, else the channel's own while it lasts
   *  (rowNowNext). Its own prop so the channel object stays the page's. */
  now: ProgrammeDto | null;
  /** The programme after the one on, from the guide; null when it has none. */
  next: ProgrammeDto | null;
  /** The time in seconds, advanced once a minute. Progress is drawn from it,
   *  and a new value is what redraws the bar through the memo. */
  minute: number;
  /** Right-click, for the lists that have a menu over a row. */
  onMenu?: (c: ChannelDto, x: number, y: number) => void;
}

export const ChannelRow = memo(function ChannelRow({
  channel,
  top,
  selected,
  pending,
  onPlay,
  onOpenFull,
  categoryName,
  now,
  next,
  minute,
  onMenu,
}: ChannelRowProps) {
  const style = { top, height: LIST_ROW_HEIGHT };

  if (channel.isSeparator) {
    // A provider-injected divider. Never playable, never a search hit.
    return (
      <div className="row sep" style={style} role="separator">
        {channel.cleanName}
      </div>
    );
  }

  const badge = qualityBadge(channel.quality);
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
      onContextMenu={
        onMenu
          ? (e) => {
              // WebKit's own menu is suppressed at the GTK level, so this
              // event arrives here in the real app exactly as it does in a
              // browser; preventDefault is for the browser-driven mock.
              e.preventDefault();
              onMenu(channel, e.clientX, e.clientY);
            }
          : undefined
      }
    >
      <span className="row-num">{channel.numLabel ?? channel.num ?? ""}</span>
      <Logo url={channel.logoUrl} />
      <span className="row-title">
        <span className="row-name">
          {/* Tips only when cut short. An instant tip on every row the
              pointer crosses would be a strobe; the name is offered where
              it is needed, which is where the ellipsis is. The raw name
              rather than the clean one, because it is the full story —
              prefix and all. */}
          <span className="row-name-text" data-tip-overflow={channel.name}>
            {channel.cleanName}
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
          {/* No badge at all when quality is null: unknown is not bad. Its
              tip is everything measured, and when. */}
          {badge ? (
            <span className="badge" data-tip={qualityDetail(channel.quality, nowSec) || undefined}>
              {badge}
            </span>
          ) : null}
        </span>
        {categoryName ? (
          <span className="row-cat" data-tip-overflow="">
            {categoryName}
          </span>
        ) : null}
      </span>
      {now ? (
        <span className="row-now">
          <span className="row-now-title" data-tip-overflow="">
            {now.title}
          </span>
          <span className="row-now-line">
            <span className="row-prog">
              <span style={{ width: `${programmeProgress(now.start, now.stop, minute) * 100}%` }} />
            </span>
            <span className="row-times">{timeRange(now.start, now.stop)}</span>
          </span>
        </span>
      ) : (
        <span className="row-now none">no guide data</span>
      )}
      <span className="row-next" data-tip-overflow="">
        {next ? `Next · ${next.title}` : ""}
      </span>
    </button>
  );
});
