// The pure half of watching a movie in a browser (src/webVod.ts): where a
// stream is asked for, how a seek is made, where in the film the picture
// is, when its place is reported, when a pause lets the connection go, and
// the subtitles' times.
//
// A movie reaches the browser one of two ways (recording::vodwatch). A file
// the browser plays as it is ("direct") is the element's own, byte ranges
// and seeking included, and its clock is the film's. Anything else is
// remuxed to MPEG-TS from a time ("remux"): the element's clock starts at
// zero where the remux started, which is the keyframe at or before the time
// asked for, so the film's time is that start plus the element's.
//
// Checked in src/__checks__/webVod.test.mjs.

export type VodMode = "direct" | "remux";

/** A remux's address: from `from` seconds into the film (whole seconds; the
 *  remux starts on the keyframe at or before it), with sound `audio` (its
 *  place in the watch answer's list) when one is chosen. */
export function vodStreamUrl(stream: string, from: number, audio: number | null): string {
  const q: string[] = [];
  const at = Math.floor(Math.max(0, from));
  if (at > 0) q.push(`from=${at}`);
  if (audio !== null) q.push(`audio=${audio}`);
  return q.length > 0 ? `${stream}?${q.join("&")}` : stream;
}

export type SeekPlan = { kind: "native"; time: number } | { kind: "reopen"; from: number };

/** How to get to `target` seconds into the film. A direct file is the
 *  element's to seek. A remux started at `from` can only be seeked within
 *  what the element holds (`buffered`, its own clock); anywhere else is a
 *  new remux from there. */
export function seekPlan(
  mode: VodMode,
  target: number,
  from: number,
  buffered: readonly (readonly [number, number])[],
): SeekPlan {
  if (mode === "direct") return { kind: "native", time: target };
  const time = target - from;
  const held = buffered.some(([s, e]) => time >= s && time <= e);
  return held ? { kind: "native", time } : { kind: "reopen", from: target };
}

/** Where in the film the picture is: the element's clock, plus where a
 *  remux started. */
export function positionOf(mode: VodMode, from: number, currentTime: number): number {
  return mode === "remux" ? from + currentTime : currentTime;
}

/** How far ahead the element holds data, in the film's time: the end of the
 *  range the picture is in, or of the next one ahead of it; `null` when it
 *  holds nothing there. */
export function bufferedEnd(
  buffered: readonly (readonly [number, number])[],
  currentTime: number,
  from: number,
): number | null {
  const r = buffered.find(([s, e]) => currentTime >= s - 0.1 && currentTime <= e) ??
    buffered.find(([s]) => s > currentTime);
  return r ? from + r[1] : null;
}

// ------------------------------------------------------------ progress

/** How often the place in a playing film is reported. */
export const PROGRESS_EVERY_MS = 10_000;

/** Whether the place reached is to be reported now: the first time after a
 *  second of the film, then every `PROGRESS_EVERY_MS` of playing, and once
 *  when it is paused. `last` is the last report. */
export function progressDue(
  last: { at: number; position: number } | null,
  now: number,
  position: number,
  paused: boolean,
): boolean {
  if (last === null) return position >= 1;
  if (paused) return position !== last.position;
  return now - last.at >= PROGRESS_EVERY_MS;
}

/** How long a film stays paused before its connection is let go: the
 *  account's connections are few, and a recording may be waiting for one.
 *  Resuming opens the film again where it was. */
export const PAUSE_RELEASE_MS = 60_000;

/** Whether a film paused since `pausedAt` (`null`: it plays) is to let its
 *  connection go. */
export function releaseDue(pausedAt: number | null, now: number): boolean {
  return pausedAt !== null && now - pausedAt >= PAUSE_RELEASE_MS;
}

// ------------------------------------------------------------- stalls

/** How long a film may take to start, from asking for its stream. */
export const OPEN_LIMIT_MS = 30_000;

/** Whether a film opened at `openedAt` and not started yet has taken too
 *  long: its stream is asked about, and opened again or given up. */
export function openDue(openedAt: number, now: number): boolean {
  return now - openedAt >= OPEN_LIMIT_MS;
}

/** How long a playing film's place may stand still. */
export const STALL_LIMIT_MS = 30_000;

/** How many times in a row a film that would not start, or stood still, is
 *  opened again before the picture says it stopped. */
export const STUCK_LIMIT = 2;

/** How much play, in seconds of the film, ends a run of being stuck: a
 *  reopened film that plays a moment and sticks again is still stuck. */
export const UNSTUCK_AFTER_S = 10;

/** The largest move of the film's place between two looks that counts as
 *  playing; more is a seek. */
const PLAY_STEP_MAX_S = 2;

/** How often in a row the film has stuck, and how much it has played since
 *  it last did. */
export interface Stuck {
  count: number;
  played: number;
}

export const NOT_STUCK: Stuck = { count: 0, played: 0 };

/** The film stuck once more: its count up, its play since then none. */
export function stuckAgain(st: Stuck): Stuck {
  return { count: st.count + 1, played: 0 };
}

/** Whether a film stuck this often in a row is given up on. */
export function stuckTooOften(st: Stuck): boolean {
  return st.count > STUCK_LIMIT;
}

/** After the playing film's place moved from `from` to `to`: the play
 *  counted, and the run of being stuck over once it comes to
 *  `UNSTUCK_AFTER_S`. A seek, or going back, is not play. */
export function stuckAfterPlay(st: Stuck, from: number, to: number): Stuck {
  const step = to - from;
  if (st.count === 0 || !(step > 0 && step <= PLAY_STEP_MAX_S)) return st;
  const played = st.played + step;
  return played >= UNSTUCK_AFTER_S ? NOT_STUCK : { count: st.count, played };
}

/** When the film's place last moved: `prev`, while it stands still and
 *  plays; now, when it moved or is paused (a pause is not a stall). */
export function movedAt(
  prev: { at: number; position: number } | null,
  now: number,
  position: number,
  paused: boolean,
): { at: number; position: number } {
  if (prev !== null && !paused && Math.abs(position - prev.position) < 0.05) return prev;
  return { at: now, position };
}

/** Whether a playing film has stood still too long since `moved`. */
export function stallDue(moved: { at: number }, now: number): boolean {
  return now - moved.at >= STALL_LIMIT_MS;
}

/** Whether an element in `readyState` wants more of the stream than it
 *  holds: less than enough to play on (`HAVE_FUTURE_DATA`). */
export function needsData(readyState: number): boolean {
  return readyState < 3;
}

// ----------------------------------------------------------- subtitles

/** How often a chosen subtitle track is asked for its newer cues. */
export const SUBTITLE_POLL_MS = 2_000;

/** A subtitle, in the film's time. */
export interface VodCue {
  start: number;
  end: number;
  text: string;
}

/** How far before the end of the last cue held the next ask starts: a cue
 *  decoded later may end earlier than one held (cues overlap), and those
 *  held already are not added twice. */
export const SUBTITLE_OVERLAP_SECONDS = 10;

/** The time to ask for cues ending after: a while before the end of the
 *  last one held, or, with none held (just chosen, or after a seek or a
 *  reopen), where the picture is. */
export function subtitleAfter(lastEnd: number | null, position: number): number {
  return lastEnd === null ? position : lastEnd - SUBTITLE_OVERLAP_SECONDS;
}

/** A cue's times on the element's clock, which starts `start` seconds into
 *  the film; `null` for one over before the stream begins. */
export function cueTimes(cue: VodCue, start: number): { start: number; end: number } | null {
  const end = cue.end - start;
  if (end <= 0) return null;
  return { start: Math.max(0, cue.start - start), end };
}

const TIME = /^(?:(\d+):)?(\d{1,2}):(\d{2})\.(\d{3})$/;

function seconds(t: string): number | null {
  const m = TIME.exec(t.trim());
  if (!m) return null;
  return Number(m[1] ?? 0) * 3600 + Number(m[2]) * 60 + Number(m[3]) + Number(m[4]) / 1000;
}

/** The cues of a WebVTT file, as the service writes them: a timing line,
 *  then the text up to a blank line. */
export function parseVtt(doc: string): VodCue[] {
  const lines = doc.replace(/\r\n?/g, "\n").split("\n");
  if (!lines[0]?.startsWith("WEBVTT")) return [];
  const out: VodCue[] = [];
  for (let i = 1; i < lines.length; i++) {
    const arrow = lines[i].indexOf("-->");
    if (arrow < 0) continue;
    const start = seconds(lines[i].slice(0, arrow));
    // Settings may follow the end time.
    const end = seconds(lines[i].slice(arrow + 3).trim().split(/\s+/)[0] ?? "");
    const text: string[] = [];
    while (i + 1 < lines.length && lines[i + 1] !== "") text.push(lines[++i]);
    if (start !== null && end !== null && end > start) out.push({ start, end, text: text.join("\n") });
  }
  return out;
}
