// "Stats for nerds": what the player is actually doing.
//
// Modelled on YouTube's, and here for the same two reasons — to debug a
// stream that is misbehaving, and to be able to see what is happening at
// all. Most of this session's playback bugs were invisible until something
// plotted them: a buffer draining, frames being dropped, the picture
// stopping for a moment. Those are the three graphs.
//
// It carries no stream URL. The whole point of a panel like this is that it
// gets screenshotted into a bug report, and an Xtream URL has the account's
// username and password in its query string.

import { useEffect, useState } from "react";
import { backend } from "../backend";
import {
  HISTORY_LENGTH,
  STATS_POLL_MS,
  bitrate,
  bytes,
  count,
  rate,
  resolutionLine,
  seconds,
  sparkline,
  text,
} from "../logic/stats";
import { useApp } from "../store";
import type { StatsDto } from "../types";

/** The graph box, in CSS pixels before zoom. Small: three of these sit in a
 *  panel that must not cover the picture it describes. */
const GRAPH_W = 180;
const GRAPH_H = 28;

export function StatsPanel({ onClose }: { onClose: () => void }) {
  const { player, nowChannel } = useApp();
  const [stats, setStats] = useState<StatsDto | null>(null);

  // The history arrives with the reading rather than being accumulated here.
  // Gathered by this component, it would begin at the moment the panel was
  // opened — and the reason to open the panel is almost always that something
  // has just happened, which by then would already be off the left of a
  // graph that had not started yet.
  const history = stats?.history ?? [];

  useEffect(() => {
    let live = true;
    const poll = () => {
      void backend
        .playerStats()
        .then((s) => live && setStats(s))
        .catch(() => {
          // Nothing playing, or the backend is not up. The panel keeps its
          // last reading rather than flashing empty every half second.
        });
    };
    poll();
    const id = window.setInterval(poll, STATS_POLL_MS);
    return () => {
      live = false;
      window.clearInterval(id);
    };
  }, []);

  // Fixed floor under the ceiling, because the useful question is "how full",
  // not "how full relative to the fullest it has been".
  const bufferCeiling = Math.max(30, ...history.map((s) => s.buffer));

  return (
    <div className="stats" role="dialog" aria-label="Stream statistics">
      <div className="stats-head">
        <span className="stats-title">Stats for nerds</span>
        <button className="stats-close" onClick={onClose} aria-label="Close statistics">
          ✕
        </button>
      </div>

      <dl className="stats-rows">
        <Row label="Channel" value={text(nowChannel?.cleanName)} />
        <Row
          label="Resolution"
          value={resolutionLine(
            player.width,
            player.height,
            stats?.containerFps,
            stats?.measuredFps,
          )}
        />
        <Row
          label="Codecs"
          value={`${text(stats?.videoCodec)} / ${text(stats?.audioCodec)}`}
        />
        <Row
          label="Decoder"
          value={`${text(stats?.hwdec)} · ${text(stats?.pixelFormat)}`}
        />
        <Row label="Colour" value={`${text(stats?.primaries)} / ${text(stats?.gamma)}`} />
        <Row
          label="Audio"
          value={
            stats?.audioSamplerate
              ? `${stats.audioSamplerate.toLocaleString()} Hz · ${count(stats.audioChannels)} ch`
              : "—"
          }
        />
        <Row
          label="Bitrate"
          value={`${bitrate(stats?.videoBitrate)} video · ${bitrate(stats?.audioBitrate)} audio`}
        />
        <Row label="Container" value={text(stats?.container)} />

        <Graph
          label="Buffer health"
          reading={`${seconds(stats?.cacheDuration, 1)}${stats?.buffering ? " · STARVED" : ""}`}
          path={sparkline(history.map((s) => s.buffer), GRAPH_W, GRAPH_H, bufferCeiling)}
          tone={stats?.buffering ? "bad" : "good"}
        />
        <Graph
          label="Network"
          reading={rate(stats?.cacheSpeed)}
          path={sparkline(history.map((s) => s.speed), GRAPH_W, GRAPH_H)}
          tone="info"
        />
        <Graph
          label="Dropped frames"
          reading={`${count(
            (stats?.droppedFrames ?? 0) + (stats?.decoderDroppedFrames ?? 0),
          )} total`}
          path={sparkline(history.map((s) => s.dropped), GRAPH_W, GRAPH_H)}
          tone="warn"
        />

        <Row label="A/V sync" value={seconds(stats?.avsync, 3)} />
        <Row label="Demuxed" value={bytes(stats?.demuxerBytes)} />
        <Row
          label="Position"
          value={`${seconds(stats?.behindLive, 1)} behind · ${seconds(stats?.bufferedBack, 0)} back`}
        />
      </dl>

      <p className="stats-note">
        {history.length < HISTORY_LENGTH
          ? `Graphs cover the last ${Math.round(
              (history.length * STATS_POLL_MS) / 1000,
            )}s of ${Math.round((HISTORY_LENGTH * STATS_POLL_MS) / 1000)}s.`
          : `Last ${Math.round((HISTORY_LENGTH * STATS_POLL_MS) / 1000)}s.`}
      </p>
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="stats-row">
      <dt>{label}</dt>
      <dd data-selectable>{value}</dd>
    </div>
  );
}

function Graph({
  label,
  reading,
  path,
  tone,
}: {
  label: string;
  reading: string;
  path: string;
  tone: "good" | "bad" | "warn" | "info";
}) {
  return (
    <div className="stats-row graph">
      <dt>{label}</dt>
      <dd>
        <svg
          className={`stats-graph ${tone}`}
          viewBox={`0 0 ${GRAPH_W} ${GRAPH_H}`}
          preserveAspectRatio="none"
          aria-hidden="true"
        >
          <path d={path} fill="none" stroke="currentColor" strokeWidth="1.5" />
        </svg>
        <span className="stats-reading" data-selectable>
          {reading}
        </span>
      </dd>
    </div>
  );
}
