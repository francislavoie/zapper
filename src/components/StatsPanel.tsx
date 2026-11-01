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
  SAMPLE_SECONDS,
  STATS_POLL_MS,
  bitrate,
  bytes,
  count,
  rate,
  resolutionLine,
  sampleAt,
  sampleX,
  seconds,
  sparkline,
  text,
} from "../logic/stats";
import { duration, span } from "../logic/format";
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
          reading={`${span(stats?.cacheDuration, 1)}${stats?.buffering ? " · STARVED" : ""}`}
          values={history.map((s) => s.buffer)}
          ceiling={bufferCeiling}
          describe={(v) => span(v, 1)}
          tone={stats?.buffering ? "bad" : "good"}
        />
        <Graph
          label="Network"
          reading={rate(stats?.cacheSpeed)}
          values={history.map((s) => s.speed)}
          describe={(v) => rate(v)}
          tone="info"
        />
        {/* Two kinds of hitch on one graph, in two colours: dropped frames
            as the line, stalls as red bands behind it. They are different
            things — a stall drops nothing, playback freezes and waits for
            data — so the drop counter alone stayed at zero through the
            stutter people notice most; side by side, which one happened
            when is one glance. */}
        <Graph
          label="Drops & stalls"
          reading={`${count(
            (stats?.droppedFrames ?? 0) + (stats?.decoderDroppedFrames ?? 0),
          )} dropped · ${count(stats?.stalls ?? 0)} ${(stats?.stalls ?? 0) === 1 ? "stall" : "stalls"}`}
          values={history.map((s) => s.dropped)}
          stalled={history.map((s) => s.starved)}
          describe={(v, i) =>
            `${count(v)} dropped${history[i]?.starved ? " · stalled" : ""}`
          }
          tone="warn"
        />

        <Row label="A/V sync" value={seconds(stats?.avsync, 3)} />
        <Row label="Demuxed" value={bytes(stats?.demuxerBytes)} />
        {/* Two rows with their own names. "0.0 s behind · 541 s back" made
            the reader work out what "back" meant; the watch band already
            calls the second one the rewind buffer, in words. */}
        <Row label="Behind live" value={span(stats?.behindLive, 1)} />
        <Row
          label="Rewind buffer"
          value={stats?.bufferedBack != null ? duration(stats.bufferedBack) : "—"}
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

/** One graph, with a readout of the latest value — or, while the pointer is
 *  over the graph, of the sample under it, marked with a line and a dot. A
 *  trace that dipped is a question ("how low, and when?"), and the answer
 *  should not need guessing from the shape. */
function Graph({
  label,
  reading,
  values,
  ceiling,
  stalled,
  describe,
  tone,
}: {
  label: string;
  reading: string;
  values: readonly number[];
  ceiling?: number;
  /** Samples at which playback was stalled, shaded as red bands behind the
   *  line: a second series in a second colour on the same time axis. */
  stalled?: readonly boolean[];
  /** One sample's value in words, for the hover readout. */
  describe: (v: number, index: number) => string;
  tone: "good" | "bad" | "warn" | "info";
}) {
  const step = GRAPH_W / Math.max(HISTORY_LENGTH - 1, 1);
  const [hover, setHover] = useState<number | null>(null);
  const index = hover !== null && hover < values.length ? hover : null;
  const top = Math.max(ceiling ?? Math.max(0, ...values), Number.EPSILON);
  const x = index !== null ? sampleX(index, values.length) * GRAPH_W : 0;
  const y =
    index !== null ? GRAPH_H - (Math.min(Math.max(values[index], 0), top) / top) * GRAPH_H : 0;
  const ago = index !== null ? (values.length - 1 - index) * SAMPLE_SECONDS : 0;

  return (
    <div className="stats-row graph">
      <dt>{label}</dt>
      <dd>
        <svg
          className={`stats-graph ${tone}`}
          viewBox={`0 0 ${GRAPH_W} ${GRAPH_H}`}
          preserveAspectRatio="none"
          aria-hidden="true"
          onPointerMove={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            setHover(r.width > 0 ? sampleAt((e.clientX - r.left) / r.width, values.length) : null);
          }}
          onPointerLeave={() => setHover(null)}
        >
          {stalled
            ? stalled.map((s, i) =>
                s ? (
                  <rect
                    key={i}
                    className="stats-stall"
                    x={sampleX(i, stalled.length) * GRAPH_W - step / 2}
                    y={0}
                    width={step}
                    height={GRAPH_H}
                  />
                ) : null,
              )
            : null}
          <path
            d={sparkline(values, GRAPH_W, GRAPH_H, ceiling)}
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
          />
          {index !== null ? (
            <g className="stats-cursor">
              <line x1={x} x2={x} y1={0} y2={GRAPH_H} vectorEffect="non-scaling-stroke" />
              <circle cx={x} cy={y} r={2.5} />
            </g>
          ) : null}
        </svg>
        <span className="stats-reading" data-selectable>
          {index !== null
            ? `${describe(values[index], index)} · ${ago === 0 ? "now" : `${duration(ago)} ago`}`
            : reading}
        </span>
      </dd>
    </div>
  );
}
