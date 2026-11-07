// The pure half of watching in a browser (src/webPlayer.ts): whether this
// browser can play at all, what it tells the service it decodes, where in
// its buffer the picture is, what the service's refusals become on the
// picture, and the warning about a recording that needs the connection.
//
// The service sends MPEG-TS, which the page's player (mpegts.js) turns into
// what Media Source Extensions take. No MSE, no playback: iPhones and iPads
// before iOS 17.1 have none (17.1 brought Apple's "managed" variant).
//
// Checked in src/__checks__/webPlayback.test.mjs.

import type { AspectMode, EndingKind, StreamEnding } from "../types";

/** What a browser offers to judge playback by: `MediaSource` or Apple's
 *  `ManagedMediaSource`, either with `isTypeSupported`. */
export interface MseHost {
  MediaSource?: { isTypeSupported?: (type: string) => boolean };
  ManagedMediaSource?: { isTypeSupported?: (type: string) => boolean };
}

/** H.264 and AAC in fragmented MP4: what every channel the service lets
 *  through becomes. */
const BASELINE = 'video/mp4; codecs="avc1.42E01E,mp4a.40.2"';

function typeCheck(host: MseHost): ((t: string) => boolean) | null {
  const ms = host.MediaSource ?? host.ManagedMediaSource;
  const f = ms?.isTypeSupported;
  if (typeof f !== "function") return null;
  return (t) => {
    try {
      return f.call(ms, t);
    } catch {
      return false;
    }
  };
}

/** Whether live TV can play in this browser. */
export function canPlayHere(host: MseHost): boolean {
  const check = typeCheck(host);
  return check !== null && check(BASELINE);
}

/** Said where it can't. */
export const UNSUPPORTED =
  "This browser can’t play live TV: it lacks Media Source Extensions (on an iPhone or iPad, iOS 17.1 or later has them). Open the channel in the zapper app, or record it.";

/** What the browser decodes beyond H.264 and AAC, for the service to decide
 *  what to convert (recording::tsprobe). */
export interface BrowserCan {
  hevc: boolean;
  ac3: boolean;
  eac3: boolean;
}

export function browserCan(host: MseHost): BrowserCan {
  const check = typeCheck(host) ?? (() => false);
  return {
    hevc:
      check('video/mp4; codecs="hvc1.1.6.L93.B0"') || check('video/mp4; codecs="hev1.1.6.L93.B0"'),
    ac3: check('audio/mp4; codecs="ac-3"'),
    eac3: check('audio/mp4; codecs="ec-3"'),
  };
}

// ------------------------------------------------------------ the buffer

/** How long the browser keeps behind the picture, at most: what can be
 *  rewound. The player clears what is older. */
export const SCROLLBACK_SECONDS = 150;

/** Paused this far behind live, the stream is let go (it would otherwise
 *  keep filling the browser's memory, and keep the account's connection);
 *  resuming starts again at live. */
export const PAUSE_LIMIT_SECONDS = 180;

/** How far behind the newest frame "live" is played: a few seconds, so a late
 *  packet does not stall the picture. */
export const LIVE_MARGIN = 3;

/** What is held before the picture starts: as much as live is played
 *  behind. */
export const START_AHEAD_SECONDS = LIVE_MARGIN;

/** Where the picture is in what the browser holds: seconds of it behind,
 *  and how far behind the newest frame. `ranges` are the video element's
 *  buffered ranges, `[start, end]`, in order. */
export function bufferPosition(
  ranges: readonly (readonly [number, number])[],
  current: number,
): { bufferedBack: number; behindLive: number } {
  if (ranges.length === 0) return { bufferedBack: 0, behindLive: 0 };
  // The range the picture is in, or the nearest before it.
  let i = ranges.findIndex(([s, e]) => current >= s - 0.1 && current <= e + 0.1);
  if (i < 0) i = ranges.length - 1;
  const end = ranges[ranges.length - 1][1];
  return {
    bufferedBack: Math.max(0, current - ranges[i][0]),
    behindLive: Math.max(0, end - LIVE_MARGIN - current),
  };
}

/** Whether playback counts as live, with the app's hysteresis: it stays live
 *  until 8 s behind, and becomes live again within 3 s. */
export function atLiveEdge(wasLive: boolean, behindLive: number): boolean {
  return wasLive ? behindLive <= 8 : behindLive <= 3;
}

/** Where a skip of `by` seconds from `current` lands: inside what is held,
 *  never past live. `null` when nothing is held. */
export function seekTarget(
  ranges: readonly (readonly [number, number])[],
  current: number,
  by: number,
): number | null {
  if (ranges.length === 0) return null;
  const start = ranges.find(([, e]) => e >= current)?.[0] ?? ranges[0][0];
  const end = ranges[ranges.length - 1][1] - LIVE_MARGIN;
  return Math.max(start + 0.2, Math.min(end, current + by));
}

/** The live edge, to jump to. */
export function liveTarget(ranges: readonly (readonly [number, number])[]): number | null {
  if (ranges.length === 0) return null;
  const [start, end] = ranges[ranges.length - 1];
  return Math.max(start, end - LIVE_MARGIN);
}

// ------------------------------------------------------------ refusals

/** What the service says when it won't start a stream (recording::watch). */
export interface WatchRefusal {
  error?: string;
  reason?: string;
  anyway?: boolean;
  slots?: { active: number; max: number } | null;
  ending?: { kind: string; status: number | null; limit: boolean; clipSecs?: number | null } | null;
  watchInstead?: number[];
}

const ENDING_KINDS: readonly EndingKind[] = [
  "dropped",
  "refused",
  "notVideo",
  "unavailable",
  "serverError",
  "network",
  "noVideo",
  "other",
  "inUse",
  "offAir",
];

function ending(kind: EndingKind, from: Partial<StreamEnding> = {}): StreamEnding {
  return { kind, slots: null, status: null, limit: false, clipSecs: null, ...from };
}

/** The ending the service reported, in the player's terms. */
export function endingOf(e: WatchRefusal["ending"]): StreamEnding | null {
  if (!e || !(ENDING_KINDS as readonly string[]).includes(e.kind)) return null;
  return ending(e.kind as EndingKind, {
    status: e.status ?? null,
    limit: e.limit === true,
    clipSecs: typeof e.clipSecs === "number" ? e.clipSecs : null,
  });
}

/** What the picture shows for a refusal: the reason as the app's player
 *  would put it where it is the same thing (another device has the
 *  account's connection: "Watch here anyway"), and the service's own words
 *  otherwise, under a headline of their own (the first line: what the
 *  docked picture has room for). */
export function refusalState(
  status: number,
  body: WatchRefusal | null,
): { ending: StreamEnding | null; error: string } {
  const said = body?.error?.trim() || null;
  switch (body?.reason) {
    case "inUse":
    case "viewers":
      return {
        ending: ending("inUse", { slots: body.slots ?? null }),
        error: said ?? "Your account is in use on another device.",
      };
    case "notPlayable":
      return { ending: null, error: `Not playable in the browser.\n${said ?? ""}`.trim() };
    case "recording":
      return { ending: null, error: `The account's connection is recording.\n${said ?? ""}`.trim() };
    case "feed": {
      const e = endingOf(body.ending);
      return { ending: e ?? ending("other"), error: said ?? "The channel didn't start." };
    }
    default:
      if (status === 0) return { ending: ending("network"), error: "The recording service could not be reached." };
      return { ending: null, error: said ?? `The recording service answered ${status}.` };
  }
}

/** Whether pressing play on the same channel again means "Watch here
 *  anyway": the picture offered exactly that (logic/ending.ts) for this
 *  ending. */
export function meansAnyway(e: StreamEnding | null): boolean {
  if (!e) return false;
  return e.kind === "inUse" || e.kind === "refused" || e.kind === "dropped" || e.kind === "notVideo";
}

// ---------------------------------------------------------- the warning

/** A recording on another channel that needs the connection this browser
 *  is watching on (`NoticeDto`). */
export interface WatchNotice {
  recordingId: number;
  title: string;
  channel: string;
  streamId: number;
  /** Its padded start, and its programme's start. */
  from: number;
  start: number;
  /** It has started, and waits for this connection. */
  waiting: boolean;
}

/** The warning's sentence. `time` writes a time of day (logic/format.ts). */
export function noticeText(n: WatchNotice, time: (unix: number) => string): string {
  return n.waiting
    ? `“${n.title}” can’t start recording on ${n.channel} while you watch: it needs this connection.`
    : `“${n.title}” starts recording on ${n.channel} at ${time(n.start)} and needs this connection.`;
}

// -------------------------------------------------------- aspect ratio

const FORCED: Partial<Record<AspectMode, number>> = {
  "16:9": 16 / 9,
  "4:3": 4 / 3,
  "2.35:1": 2.35,
};

/** How the picture is fitted to its box for an aspect choice: the CSS
 *  `object-fit`, and a scale on top of it that turns the picture's own
 *  shape (`native`, width over height) into a forced one, inside the box. */
export function aspectFit(
  mode: AspectMode,
  box: { width: number; height: number },
  native: number,
): { fit: "contain" | "cover" | "fill"; sx: number; sy: number } {
  if (mode === "fill") return { fit: "cover", sx: 1, sy: 1 };
  if (mode === "stretch") return { fit: "fill", sx: 1, sy: 1 };
  const want = FORCED[mode];
  if (!want || !(native > 0) || box.width <= 0 || box.height <= 0) {
    return { fit: "contain", sx: 1, sy: 1 };
  }
  // Contained at its own shape, the picture is w0 × w0/native; at the
  // forced one, w1 × w1/want.
  const w0 = Math.min(box.width, box.height * native);
  const w1 = Math.min(box.width, box.height * want);
  return { fit: "contain", sx: w1 / w0, sy: w1 / want / (w0 / native) };
}
