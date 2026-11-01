// Recordings, as the guide and the Recordings dialog show them: which
// programmes carry the red dot, what scheduling one sends, and how the list
// is grouped and worded.
//
// The recordings themselves are the recording service's (zapper-server, see
// docs/recording.md); this is only what the interface makes of them.
//
// Checked in src/__checks__/recordings.test.mjs.

import type {
  ChannelDto,
  ProgrammeDto,
  RecordingDto,
  RecordingState,
  ScheduleRequest,
} from "../types";
import { duration, timeRange } from "./format.ts";
import { dayLabel } from "./guide.ts";

/** A recording is found by its channel and its programme's start, which is
 *  also how the service refuses to schedule the same airing twice. */
export const markKey = (streamId: number, start: number) => `${streamId}:${start}`;

/** The recordings that still mark a programme in the guide: those waiting
 *  to start and those being captured. Finished ones are the list's, not the
 *  guide's. */
export function marks(recs: readonly RecordingDto[]): Map<string, RecordingDto> {
  const out = new Map<string, RecordingDto>();
  for (const r of recs) {
    if (r.state === "scheduled" || r.state === "recording") out.set(markKey(r.streamId, r.start), r);
  }
  return out;
}

/** Whether a programme can still be recorded: anything not yet over. One on
 *  now records from now on, missing what has already aired. */
export function canRecord(p: Pick<ProgrammeDto, "stop">, now: number): boolean {
  return p.stop > now;
}

export function scheduleRequest(ch: ChannelDto, p: ProgrammeDto): ScheduleRequest {
  return {
    streamId: ch.streamId,
    channelName: ch.cleanName || ch.name,
    title: p.title,
    start: p.start,
    stop: p.stop,
  };
}

const ORDER: { state: RecordingState; label: string }[] = [
  { state: "recording", label: "Recording" },
  { state: "scheduled", label: "Scheduled" },
  { state: "done", label: "Recorded" },
  { state: "failed", label: "Failed" },
];

export interface RecordingGroup {
  state: RecordingState;
  label: string;
  items: RecordingDto[];
}

/** Recording now first, then what is coming up (soonest first), then what
 *  has been recorded (newest first), then failures (newest first). Empty
 *  groups are left out. */
export function groupRecordings(recs: readonly RecordingDto[]): RecordingGroup[] {
  return ORDER.map(({ state, label }) => {
    const items = recs.filter((r) => r.state === state);
    items.sort((a, b) => (state === "scheduled" || state === "recording" ? a.start - b.start : b.start - a.start));
    return { state, label, items };
  }).filter((g) => g.items.length > 0);
}

/** "Today 20:00–21:00", "Tomorrow 06:30–07:00", "Tue 21:00–22:15", on the
 *  clock Settings › Interface chooses. */
export function recordingWhen(r: Pick<RecordingDto, "start" | "stop">, now: number): string {
  return `${dayLabel(r.start, now) || "Today"} ${timeRange(r.start, r.stop)}`;
}

/** What the row says under its title. */
export function recordingStatus(r: RecordingDto, now: number): string {
  const size = bytes(r.parts.reduce((n, p) => n + p.bytes, 0));
  const parts = r.parts.length > 1 ? ` in ${r.parts.length} parts` : "";
  switch (r.state) {
    case "scheduled": {
      const from = r.start - r.padBefore;
      return from > now ? `starts in ${duration(from - now)}` : "starting";
    }
    case "recording":
      return r.waiting ? capitalise(r.waiting) : `${size} so far${parts}`;
    case "done":
      return `${size}${parts}`;
    case "failed":
      return r.error ? capitalise(r.error) : "Nothing was recorded";
  }
}

function capitalise(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/** "740 MB", "1.2 GB": decimal units, as file managers show them. */
export function bytes(n: number): string {
  if (n < 1000) return `${n} B`;
  const units = ["kB", "MB", "GB", "TB"];
  let v = n;
  let i = -1;
  while (v >= 1000 && i < units.length - 1) {
    v /= 1000;
    i++;
  }
  return `${v >= 100 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

/** The service's defaults (src-tauri/src/recording/config.rs), for the ↺
 *  beside each of Settings › Recording's rows. The folder's default comes
 *  from the service itself, since it depends on the machine. */
export const RECORDING_DEFAULTS = {
  padBefore: 60,
  padAfter: 300,
  clockTrack: true,
} as const;

/** Minutes from the seconds the service keeps padding in, and back. */
export const padMinutes = (seconds: number) => Math.round(seconds / 60);
export const padSeconds = (minutes: number) => Math.max(0, Math.round(minutes)) * 60;
