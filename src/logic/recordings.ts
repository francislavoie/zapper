// Recordings, as the guide and the Recordings pane show them: which
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
  WhileRecording,
} from "../types";
import { duration, hhmm, timeRange } from "./format.ts";
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

/** The programme on now on the channel the player shows, for Record from
 *  the player: on by the guide, and the stream live (`watching`, the
 *  programme the player was opened for, has not ended: a catch-up of
 *  something already aired is not what is on). */
export function programmeOnNow(
  onNow: ProgrammeDto | null,
  watching: ProgrammeDto | null,
  now: number,
): ProgrammeDto | null {
  if (!onNow) return null;
  if (watching && watching.stop <= now) return null;
  if (onNow.start > now || onNow.stop <= now) return null;
  return onNow;
}

/** What Record from the player (its menu, the now-playing panel, R) offers
 *  for what it shows: to record the programme on now; or, when it is
 *  already scheduled or recording, that recording (to stop or cancel); or,
 *  while it is being saved, that. `null` when there is nothing to offer:
 *  no service answering (`recs` null), no programme known, or a separator. */
export type PlayerRecord =
  | { kind: "record"; channel: ChannelDto; programme: ProgrammeDto }
  | { kind: "recording" | "scheduled" | "saving"; channel: ChannelDto; rec: RecordingDto };

export function playerRecord(
  recs: readonly RecordingDto[] | null,
  channel: ChannelDto | null,
  programme: ProgrammeDto | null,
  now: number,
): PlayerRecord | null {
  if (!recs || !channel || channel.isSeparator || !programme) return null;
  const rec = marks(recs).get(markKey(channel.streamId, programme.start));
  if (rec) {
    const kind = rec.state === "recording" ? "recording" : rec.state === "finishing" ? "saving" : "scheduled";
    return { kind, channel, rec };
  }
  if (!canRecord(programme, now)) return null;
  return { kind: "record", channel, programme };
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

export type RecordingsFilter = "all" | "scheduled" | "recorded";

export interface RecordingSection {
  id: "now" | "scheduled" | "recorded";
  label: string;
  items: RecordingDto[];
}

/** The Recordings pane's one list: what is recording or being saved, then
 *  what is coming up (soonest first), then what has been recorded or
 *  failed (newest first). The filter keeps the first two ("scheduled") or
 *  only the last; empty sections are left out. */
export function recordingSections(recs: readonly RecordingDto[], filter: RecordingsFilter): RecordingSection[] {
  const pick = (states: RecordingState[], newestFirst: boolean) =>
    recs
      .filter((r) => states.includes(r.state))
      .sort((a, b) => (newestFirst ? b.start - a.start : a.start - b.start) || a.id - b.id);
  const all: RecordingSection[] = [
    { id: "now", label: "Recording now", items: pick(["recording", "finishing"], false) },
    { id: "scheduled", label: "Scheduled", items: pick(["scheduled"], false) },
    { id: "recorded", label: "Recorded", items: pick(["done", "failed"], true) },
  ];
  return all.filter((s) => s.items.length > 0 && (filter === "all" || (filter === "recorded") === (s.id === "recorded")));
}

/** The selected recording, or none when the filter hides it. */
export function visibleSelection(sections: readonly RecordingSection[], id: number | null): number | null {
  return id !== null && sections.some((s) => s.items.some((r) => r.id === id)) ? id : null;
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

/** A scheduled recording this near (padded start) is worth saying so. */
export const SOON_SECS = 12 * 3600;

/** What a row's Status column says: how its recording is going, with a
 *  progress bar while it records. `tone` colours it: a conflict or a
 *  connection it waits for is a warning, a failure is bad, a finished one
 *  good. */
export interface RowStatus {
  text: string;
  tone: "" | "warn" | "bad" | "good";
  progress: number | null;
}

const lowerFirst = (s: string) => s.charAt(0).toLowerCase() + s.slice(1);

/** For a scheduled row, conflicts first, then how soon it starts, then its
 *  backup channel; for the others, how far along or how it ended. */
export function recordingRowStatus(r: RecordingDto, now: number): RowStatus {
  const size = bytes(r.parts.reduce((n, p) => n + p.bytes, 0));
  switch (r.state) {
    case "scheduled":
    case "recording": {
      const from = r.start - r.padBefore;
      const until = r.stop + r.padAfter;
      const conflict = r.conflicts?.[0];
      if (r.state === "recording") {
        if (r.waiting) return { text: capitalise(r.waiting), tone: "warn", progress: null };
        const progress = Math.min(1, Math.max(0, (now - from) / (until - from)));
        return { text: `recording · ${duration(Math.max(0, until - now))} left`, tone: "", progress };
      }
      if (conflict) return { text: lowerFirst(conflictText(conflict)), tone: "warn", progress: null };
      if (from <= now) return { text: "starting", tone: "", progress: null };
      if (from - now < SOON_SECS) return { text: `starts in ${duration(from - now)}`, tone: "", progress: null };
      if (r.backupStreamId != null)
        return { text: `backup: ${r.backupName ?? `channel ${r.backupStreamId}`}`, tone: "", progress: null };
      return { text: "", tone: "", progress: null };
    }
    case "finishing":
      return { text: `saving ${size}…`, tone: "", progress: null };
    case "done": {
      const n = r.parts.length;
      return { text: `✓ ${n === 1 ? "1 file" : `${n} parts`} · ${size}`, tone: "good", progress: null };
    }
    case "failed":
      return { text: r.error ? capitalise(r.error) : "Nothing was recorded", tone: "bad", progress: null };
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

/** The title bar's indicator while the service records: how many, in a
 *  word, and the tooltip listing each. */
export interface RecordingIndicator {
  /** "Recording", "Recording 2", or "Saving" when all that is left is
   *  being saved into files. */
  label: string;
  /** Red and pulsing while anything records; grey while only saving. */
  tone: "live" | "saving";
  /** One line per recording; see `activeRecordingLine`. */
  tip: string;
}

/** Whole minutes from a minute on: "23m", "1h05m"; seconds before that. */
function elapsed(secs: number): string {
  return duration(secs >= 60 ? Math.floor(secs / 60) * 60 : secs);
}

/** "Channel · Title — 23m, until 21:05", and on lines of their own what is
 *  worth knowing: waiting for a connection, a conflict ahead, the backup
 *  channel in use. A recording being saved says so instead of the times. */
export function activeRecordingLine(r: RecordingDto, now: number): string {
  const head = `${r.channelName} · ${r.title}`;
  if (r.state === "finishing") return `${head} — saving into its file…`;
  const began = r.parts[0]?.started ?? r.start - r.padBefore;
  const lines = [`${head} — ${elapsed(Math.max(0, now - began))}, until ${hhmm(r.stop + r.padAfter)}`];
  // The tooltip collapses leading spaces, so the details are marked instead
  // of indented.
  if (r.waiting) lines.push(`↳ ${capitalise(r.waiting)}`);
  const feed = r.parts[r.parts.length - 1]?.streamId;
  if (feed != null && feed !== r.streamId) {
    lines.push(`↳ On its backup channel${r.backupName ? `, ${r.backupName}` : ""}`);
  }
  const conflict = r.conflicts?.[0];
  if (conflict) lines.push(`↳ ${conflictText(conflict)}`);
  return lines.join("\n");
}

/** What the title bar shows, or `null` — nothing recording or being saved,
 *  or no service answering (`recs` null) — for no indicator at all. */
export function recordingIndicator(
  recs: readonly RecordingDto[] | null,
  now: number,
): RecordingIndicator | null {
  if (!recs) return null;
  const recording = recs.filter((r) => r.state === "recording");
  const saving = recs.filter((r) => r.state === "finishing");
  if (recording.length === 0 && saving.length === 0) return null;
  const label =
    recording.length === 0 ? "Saving" : recording.length === 1 ? "Recording" : `Recording ${recording.length}`;
  const order = (a: RecordingDto, b: RecordingDto) => a.start - b.start || a.id - b.id;
  const tip = [...recording.sort(order), ...saving.sort(order)]
    .map((r) => activeRecordingLine(r, now))
    .join("\n");
  return { label, tone: recording.length > 0 ? "live" : "saving", tip };
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
 *  something was asked of the service; every few seconds while the Recordings
 *  pane shows; otherwise every half minute, which is what keeps the guide's
 *  dots current. */
export function pollDelay(
  recs: readonly Pick<RecordingDto, "state">[] | null,
  shown: boolean,
  sinceTouchedMs: number,
): number {
  const busy = recs?.some((r) => r.state === "recording" || r.state === "finishing") ?? false;
  if (busy || (sinceTouchedMs >= 0 && sinceTouchedMs < TOUCHED_MS)) return POLL_BUSY_MS;
  return shown ? POLL_OPEN_MS : POLL_IDLE_MS;
}

/** The service's defaults (src-tauri/src/recording/config.rs), for the ↺
 *  beside each of Settings › Recording's rows. The folder's default comes
 *  from the service itself, since it depends on the machine. */
export const RECORDING_DEFAULTS = {
  naming: "mediaServer" as Naming,
  padBefore: 60,
  padAfter: 300,
  clockTrack: true,
  whileRecording: "hidden" as WhileRecording,
  strmHost: "",
} as const;

/** The choices of "Show recordings to other programs while they record". */
export const WHILE_RECORDING: readonly { id: WhileRecording; label: string; tip: string }[] = [
  { id: "hidden", label: "No", tip: "Only the finished .mkv appears" },
  {
    id: "visible",
    label: "Growing file",
    tip: "The file being recorded, as “… (recording).ts”",
  },
  {
    id: "strm",
    label: "Jellyfin (.strm)",
    tip: "A “[RECORDING] ….strm” pointing at the service’s live stream of the recording",
  },
];

/** What the `.strm` address needs said about it, from where it points
 *  (`strmBase`) and whether a host was chosen: nothing; that only this
 *  computer reaches it; or that it is plain HTTP across the network. */
export function strmReach(strmBase: string, hostChosen: boolean): "fine" | "local" | "plain" {
  let host = "";
  try {
    host = new URL(strmBase).hostname;
  } catch {
    return "fine";
  }
  const local = host === "127.0.0.1" || host === "localhost" || host === "[::1]";
  if (local) return hostChosen ? "fine" : "local";
  return strmBase.startsWith("http://") ? "plain" : "fine";
}

/** Minutes from the seconds the service keeps padding in, and back. */
export const padMinutes = (seconds: number) => Math.round(seconds / 60);
export const padSeconds = (minutes: number) => Math.max(0, Math.round(minutes)) * 60;

/** The Recordings rail item's count: how many are capturing now and how many
 *  are still waiting, blank when neither. */
export function recordingsRailCount(recs: readonly Pick<RecordingDto, "state">[] | null): string {
  const now = recs?.filter((r) => r.state === "recording").length ?? 0;
  const next = recs?.filter((r) => r.state === "scheduled").length ?? 0;
  return [now && `${now} now`, next && `${next} next`].filter(Boolean).join(" · ");
}

/** The selected recording, or none once it is no longer in the list. */
export function keepSelected(recs: readonly { id: number }[] | null, id: number | null): number | null {
  return id !== null && recs?.some((r) => r.id === id) ? id : null;
}
