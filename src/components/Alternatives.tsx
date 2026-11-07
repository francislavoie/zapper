// "Try another source:" — other channels with the same programmes, under the
// message for a channel that is off air (logic/ending.ts says when). Nothing
// at all when there are none: the message stands on its own.
//
// Each is played exactly as a click in the channel list plays it, through
// the same path and under the same connection budget. Buttons, so Tab
// reaches them and Enter plays one.

import { useEffect, useState } from "react";
import { playChannel } from "../actions";
import { backend } from "../backend";
import { alternativeLine } from "../logic/alternatives";
import { useDispatch } from "../store";
import type { ChannelDto } from "../types";

export function Alternatives({ streamId }: { streamId: number }) {
  const dispatch = useDispatch();
  const [found, setFound] = useState<{ for: number; channels: ChannelDto[] } | null>(null);

  useEffect(() => {
    let live = true;
    backend
      .alternativesFor(streamId)
      .then((channels) => {
        if (live) setFound({ for: streamId, channels });
      })
      // Suggestions are a courtesy: failing to find them is finding none.
      .catch(() => {
        if (live) setFound({ for: streamId, channels: [] });
      });
    return () => {
      live = false;
    };
  }, [streamId]);

  const channels = found?.for === streamId ? found.channels : [];
  if (channels.length === 0) return null;

  return (
    <div className="alternatives">
      <p className="alternatives-head">Try another source:</p>
      <ul className="alternatives-list">
        {channels.map((c) => {
          const line = alternativeLine(c);
          return (
            <li key={c.streamId}>
              <button
                className="alternative"
                onClick={(e) => {
                  // The picture under the message takes a click as "open the
                  // player"; this one is a channel.
                  e.stopPropagation();
                  void playChannel(dispatch, c);
                }}
              >
                <span className="alternative-num">{line.num}</span>
                <span className="alternative-name" data-tip-overflow={line.fullName}>
                  {line.name}
                </span>
                {line.resolution ? <span className="badge">{line.resolution}</span> : null}
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
