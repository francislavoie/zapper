// What is playing, shown beside the docked picture while browsing. It sits
// below the dock, never over it — the video is drawn beneath the window, so a
// card on top of it would be a hole in the picture.

import { hhmm, liveOffset, measuredLine } from "../logic/format";
import { useApp, useDispatch } from "../store";

export function NowCard() {
  const { nowChannel, player } = useApp();
  const dispatch = useDispatch();

  if (!nowChannel || player.state === "stopped") {
    return (
      <div className="now-card">
        <h3>Nothing playing</h3>
        <p>Pick a channel. Browsing never stops what is already open.</p>
      </div>
    );
  }

  const measured = measuredLine(player);
  const now = nowChannel.nowPlaying;

  return (
    <div className="now-card">
      <h3>{nowChannel.cleanName}</h3>
      <p>
        {player.state === "opening"
          ? "Opening…"
          : player.state === "failed"
            ? (player.error ?? "Stream failed")
            : now
              ? `${hhmm(now.start)}–${hhmm(now.stop)}  ${now.title}`
              : "no guide data"}
      </p>
      <p>
        {measured || "resolution not measured yet"} · {liveOffset(player.behindLive)}
      </p>
      <button className="ctl" onClick={() => dispatch({ type: "mode", mode: "watch" })}>
        Back to full screen
      </button>
    </div>
  );
}
