// Recordings, as the guide and the Recordings dialog show them: which
// programmes carry the red dot, what scheduling one sends, and how the list
// is grouped and worded.
//
// The recordings themselves are the recording service's (zapper-server, see
// docs/recording.md); this is only what the interface makes of them.
//
// Checked in src/__checks__/recordings.test.mjs.

import type {
  BackupDto,
  ChannelDto,
  ConflictDto,
  Naming,
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
 *  to start, those being captured, and those being saved into their files
 *  (a grey dot). Finished ones are the list's, not the guide's. */
export function marks(recs: readonly RecordingDto[]): Map<string, RecordingDto> {
  const out = new Map<string, RecordingDto>();
  for (const r of recs) {
    if (r.state === "scheduled" || r.state === "recording" || r.state === "finishing")
      out.set(markKey(r.streamId, r.start), r);
  }
  return out;
}

/** Whether a programme can still be recorded: anything not yet over. One on
 *  now records from now on, missing what has already aired. */
export function canRecord(p: Pick<ProgrammeDto, "stop">, now: number): boolean {
  return p.stop > now;
}

/** "S02E04", "E12" (no season known), or "" — as recordings are named. */
export function episodeLabel(p: { season?: number | null; episode?: number | null }): string {
  if (p.episode == null) return "";
  const e = `E${String(p.episode).padStart(2, "0")}`;
  return p.season != null ? `S${String(p.season).padStart(2, "0")}${e}` : e;
}

/** What scheduling a programme sends. The guide's season and episode go
 *  along when it has them, so the file is named `S02E04` rather than by
 *  date; an episode without a season goes alone (`E04`). */
export function scheduleRequest(ch: ChannelDto, p: ProgrammeDto): ScheduleRequest {
  const req: ScheduleRequest = {
    streamId: ch.streamId,
    channelName: ch.cleanName || ch.name,
    title: p.title,
    start: p.start,
    stop: p.stop,
  };
  if (p.episode != null) {
    req.episode = p.episode;
    if (p.season != null) req.season = p.season;
  }
  return req;
}

/** The recordings a programme on channel `streamId` could stand in for:
 *  scheduled or recording, on another channel, overlapping it in time.
 *  Soonest first, at most `max`, so the menu stays short. */
export function backupTargets(
  recs: readonly RecordingDto[],
  streamId: number,
  p: Pick<ProgrammeDto, "start" | "stop">,
  max = 3,
): RecordingDto[] {
  return recs
    .filter(
      (r) =>
        (r.state === "scheduled" || r.state === "recording") &&
        r.streamId !== streamId &&
        r.start < p.stop &&
        p.start < r.stop,
    )
    .sort((a, b) => a.start - b.start || a.id - b.id)
    .slice(0, max);
}

/** The programme menu's entry for one of `backupTargets`: offers the
 *  channel as its backup, or, when it already is, to stop. */
export function backupMenuLabel(r: Pick<RecordingDto, "title" | "backupStreamId">, streamId: number): string {
  return r.backupStreamId === streamId
    ? `Stop using as backup for “${r.title}”`
    : `Use as backup for “${r.title}”`;
}

/** A recording's backup, in a few words, for its row in Recordings. */
export function backupText(r: Pick<RecordingDto, "backupStreamId" | "backupName">): string {
  if (r.backupStreamId == null) return "No backup channel";
  return `Backup: ${r.backupName ?? `channel ${r.backupStreamId}`}`;
}

/** A candidate as the picker lists it: "One HD · 1080p", with why it is
 *  offered when that is the programme rather than the guide channel. */
export function backupCandidateLabel(b: BackupDto): string {
  const bits = [b.name];
  if (b.height) bits.push(`${b.height}p`);
  if (b.failing) bits.push("failed last time");
  if (b.reason === "programme") bits.push("same programme");
  return bits.join(" · ");
}

const ORDER: { state: RecordingState; label: string }[] = [
  { state: "recording", label: "Recording" },
  { state: "finishing", label: "Saving" },
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
    const ahead = state === "scheduled" || state === "recording" || state === "finishing";
    items.sort((a, b) => (ahead ? a.start - b.start : b.start - a.start));
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
    case "finishing":
      return `Saving ${size}${parts}…`;
    case "done":
      return `${size}${parts}`;
    case "failed":
      return r.error ? capitalise(r.error) : "Nothing was recorded";
  }
}

/** What a conflict means, in a few words: on the recording that loses
 *  (its row in Recordings, or the programme about to be recorded), or, for
 *  a preview's `other` one, about the recording that would lose. */
export function conflictText(c: ConflictDto): string {
  const streams = `the account allows ${c.streams} ${c.streams === 1 ? "stream" : "streams"}`;
  const d = duration(c.secs);
  if (c.other) {
    const lead = `Overlaps “${c.title}” on another channel`;
    switch (c.kind) {
      case "cutEnd":
        return `${lead}: its last ${d} will be cut`;
      case "lateStart":
        return `${lead}: it will start ${d} late`;
      case "lostStart":
        return `${lead} and ${streams}: its first ${d} will be missed`;
      case "missed":
        return `${lead} and ${streams}: it can’t be recorded`;
    }
  }
  const lead = `Overlaps “${c.with}” on another channel`;
  switch (c.kind) {
    case "cutEnd":
      return `${lead}: the last ${d} will be cut`;
    case "lateStart":
      return `${lead}: starts ${d} late`;
    case "lostStart":
      return `${lead} and ${streams}: the first ${d} will be missed`;
    case "missed":
      return `${lead} and ${streams}: it can’t be recorded`;
  }
}

/** Whether a conflict costs part of a programme, not only padding. */
export const conflictLosesProgramme = (c: ConflictDto) =>
  c.kind === "lostStart" || c.kind === "missed";

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

/** How often the recordings are fetched (ms). */
export const POLL_IDLE_MS = 30_000;
export const POLL_OPEN_MS = 4_000;
export const POLL_BUSY_MS = 2_000;
/** How long after something is asked of the service it is followed
 *  closely: a recording that was just scheduled starts, one stopped is
 *  saved, and the list should say so without waiting half a minute. */
export const TOUCHED_MS = 10_000;

/** How long to wait before fetching the recordings again: often while any
 *  is recording or being saved (its size grows, it finishes) and just after
 *  something was asked of the service; every few seconds while Recordings
 *  is open; otherwise every half minute, which is what keeps the guide's
 *  dots current. */
export function pollDelay(
  recs: readonly Pick<RecordingDto, "state">[] | null,
  dialogOpen: boolean,
  sinceTouchedMs: number,
): number {
  const busy = recs?.some((r) => r.state === "recording" || r.state === "finishing") ?? false;
  if (busy || (sinceTouchedMs >= 0 && sinceTouchedMs < TOUCHED_MS)) return POLL_BUSY_MS;
  return dialogOpen ? POLL_OPEN_MS : POLL_IDLE_MS;
}

/** The service's defaults (src-tauri/src/recording/config.rs), for the ↺
 *  beside each of Settings › Recording's rows. The folder's default comes
 *  from the service itself, since it depends on the machine. */
export const RECORDING_DEFAULTS = {
  naming: "mediaServer" as Naming,
  padBefore: 60,
  padAfter: 300,
  clockTrack: true,
  showWhileRecording: false,
} as const;

/** Minutes from the seconds the service keeps padding in, and back. */
export const padMinutes = (seconds: number) => Math.round(seconds / 60);
export const padSeconds = (minutes: number) => Math.max(0, Math.round(minutes)) * 60;
