// Display formatting. Pure; exercised by src/__checks__/format.test.mjs.

import type { PlayerStateDto, QualityDto } from "../types";

/** The badge shown on a channel row. `null` quality means never opened, which
 *  is unknown rather than bad, so it renders nothing at all. Channel names lie
 *  about resolution (`CA: HARBOUR SPORTS 1 4K` may well be 720p); only
 *  measured height is allowed to speak here. */
export function qualityBadge(
  q: Pick<QualityDto, "height" | "isHdr"> & { interlaced?: boolean | null } | null,
): string | null {
  if (!q) return null;
  const h = q.height;
  // Fields, not frames: "1080i" is what a broadcaster would call it, and it
  // is a different picture from 1080p however the deinterlacer does.
  const scan = q.interlaced ? "i" : "p";
  let label: string | null = null;
  if (h != null) {
    if (h >= 2000) label = "4K";
    else if (h >= 1400) label = `1440${scan}`;
    else if (h >= 1000) label = `1080${scan}`;
    else if (h >= 700) label = `720${scan}`;
    else if (h >= 500) label = `576${scan}`;
    else label = `${h}${scan}`;
  }
  if (q.isHdr) label = label ? `${label} HDR` : "HDR";
  return label;
}

/** How long ago an instant was, in words for a tooltip: "just now",
 *  "12m ago", "3h ago", "yesterday", "5 days ago", "2 months ago". Never a
 *  date: these say how fresh a record is, and the distance is the point. */
export function ago(unixSeconds: number, nowSeconds: number): string {
  const s = Math.max(0, nowSeconds - unixSeconds);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  const days = Math.floor(s / 86400);
  if (days === 1) return "yesterday";
  if (days < 45) return `${days} days ago`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months} months ago`;
  return "over a year ago";
}

const VIDEO_CODECS: Record<string, string> = {
  h264: "H.264",
  hevc: "HEVC",
  h265: "HEVC",
  av1: "AV1",
  vp9: "VP9",
  mpeg2video: "MPEG-2",
  mpeg4: "MPEG-4",
  vc1: "VC-1",
};

const AUDIO_CODECS: Record<string, string> = {
  aac: "AAC",
  aac_latm: "AAC",
  ac3: "AC-3",
  eac3: "E-AC-3",
  mp2: "MP2",
  mp3: "MP3",
  opus: "Opus",
  dts: "DTS",
  truehd: "TrueHD",
  flac: "FLAC",
};

/** Speaker layouts by channel count, as they are sold. */
function layout(channels: number): string {
  if (channels === 1) return "mono";
  if (channels === 2) return "stereo";
  if (channels === 6) return "5.1";
  if (channels === 8) return "7.1";
  return `${channels}ch`;
}

/** Everything measured about a channel, for its badge's tooltip:
 *  "1920×1080 · 50 fps · H.264 · interlaced · AAC stereo · checked 3h ago".
 *  Empty when nothing has been measured. */
export function qualityDetail(q: QualityDto | null, nowSeconds: number): string {
  if (!q || q.measuredAt == null) return "";
  const bits: string[] = [];
  if (q.width != null && q.height != null) bits.push(`${q.width}×${q.height}`);
  if (q.fps != null) bits.push(`${trimFps(q.fps)} fps`);
  if (q.codec) bits.push(VIDEO_CODECS[q.codec] ?? q.codec);
  if (q.interlaced) bits.push("interlaced");
  if (q.isHdr) bits.push("HDR");
  const audio = [
    q.audioCodec ? (AUDIO_CODECS[q.audioCodec] ?? q.audioCodec) : null,
    q.audioChannels != null ? layout(q.audioChannels) : null,
  ].filter((a) => a !== null);
  if (audio.length > 0) bits.push(audio.join(" "));
  bits.push(`checked ${ago(q.measuredAt, nowSeconds)}`);
  return bits.join(" · ");
}

/** Whether a channel's last open failed to load and nothing has played on
 *  it since. Only genuine failures are recorded — the account's connection
 *  limit and the network are not held against a channel. */
export function failedToLoad(q: QualityDto | null): boolean {
  return q?.failedAt != null;
}

/** The reason the backend records a channel that was off air with
 *  (`ending::OFF_AIR_REASON` in Rust): the provider sent a placeholder clip
 *  instead of it. Spelled the same on both sides. */
export const OFF_AIR_REASON = "off air";

/** Whether a channel's last live open found it off air, and nothing has
 *  played on it since. */
export function offAir(q: QualityDto | null): boolean {
  return q?.failedAt != null && q.failReason === OFF_AIR_REASON;
}

/** Whether two instants fall on the same local calendar day. */
function sameLocalDay(a: number, b: number): boolean {
  return new Date(a * 1000).toDateString() === new Date(b * 1000).toDateString();
}

/** "Off air · 18:26": when a channel was found off air, for its row and the
 *  band. The clock time on the day it happened, which is when it reads as
 *  today's; how long ago after that ("Off air · yesterday"). `null` unless
 *  the channel is off air. */
export function offAirLabel(q: QualityDto | null, nowSeconds: number): string | null {
  if (!q || q.failedAt == null || !offAir(q)) return null;
  const when = sameLocalDay(q.failedAt, nowSeconds) ? hhmm(q.failedAt) : ago(q.failedAt, nowSeconds);
  return `Off air · ${when}`;
}

/** What a dimmed row says on hover: when it last failed and why. */
export function failedTip(q: QualityDto | null, nowSeconds: number): string | null {
  if (!q || q.failedAt == null) return null;
  const off = offAirLabel(q, nowSeconds);
  if (off) return `${off} — your provider sent a placeholder instead of the channel`;
  const why = q.failReason ? ` — ${q.failReason}` : "";
  return `Didn't load ${ago(q.failedAt, nowSeconds)}${why}`;
}

/** True when a channel has been tried and has failed more often than not.
 *  Never true for an unopened channel: no attempts is not a bad record. */
export function looksBroken(q: QualityDto | null): boolean {
  if (!q) return false;
  const tries = q.okCount + q.failCount;
  return tries >= 3 && q.failCount > q.okCount;
}

/** The measured-truth line the player reveals on hover. Empty string when the
 *  player has measured nothing yet. */
export function measuredLine(p: PlayerStateDto): string {
  const bits: string[] = [];
  if (p.width != null && p.height != null) bits.push(`${p.width}×${p.height}`);
  if (p.fps != null) bits.push(`${trimFps(p.fps)} fps`);
  if (p.codec) bits.push(p.codec);
  return bits.join(" · ");
}

function trimFps(fps: number): string {
  const r = Math.round(fps * 100) / 100;
  return Number.isInteger(r) ? String(r) : r.toFixed(2).replace(/0+$/, "");
}

/** How far into a title the player is, 0–1; 0 while its length is not
 *  known. */
export function playedFraction(at: { position: number; duration: number | null }): number {
  return at.duration && at.duration > 0 ? Math.min(1, Math.max(0, at.position / at.duration)) : 0;
}

/** "23:10 / 52:00", or the position alone while the length is not known. */
export function positionLabel(at: { position: number; duration: number | null }): string {
  return at.duration ? `${clock(at.position)} / ${clock(at.duration)}` : clock(at.position);
}

/** mm:ss, or h:mm:ss past an hour. Negative clamps to zero — behindLive can
 *  arrive fractionally negative from the demuxer. */
export function clock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const mm = h > 0 ? String(m).padStart(2, "0") : String(m);
  return h > 0
    ? `${h}:${mm}:${String(sec).padStart(2, "0")}`
    : `${mm}:${String(sec).padStart(2, "0")}`;
}

/** A span of time in words: "42 s", "4 min", "10 min 40 s", "1 h 05 min".
 *
 *  For durations someone reads rather than a position someone scrubs to —
 *  "640 s" makes the reader do the division, and "10:40" reads as a time of
 *  day as often as a length. Rounded to the second under an hour and to the
 *  minute past it, where seconds stop meaning anything. Negative and
 *  non-finite clamp to zero. */
export function duration(seconds: number): string {
  // Compact — "11m12s", not "11 min 12 s" — because these sit in narrow
  // readouts and rows, and the unit letters are what keep it from reading
  // as a clock, which "11:12" did.
  const s = Number.isFinite(seconds) ? Math.max(0, Math.round(seconds)) : 0;
  if (s < 60) return `${s}s`;
  if (s < 3600) {
    const m = Math.floor(s / 60);
    const rest = s % 60;
    return rest === 0 ? `${m}m` : `${m}m${String(rest).padStart(2, "0")}s`;
  }
  const totalMin = Math.round(s / 60);
  const h = Math.floor(totalMin / 60);
  const m = totalMin % 60;
  return m === 0 ? `${h}h` : `${h}h${String(m).padStart(2, "0")}m`;
}

/** How long until something starts, to the minute: "54m", "2h05m". Seconds
 *  are noise on a countdown that is only re-read each minute, and they made
 *  "starts in 54m01s" tick like a stopwatch. Null under a minute, where there
 *  is nothing to count and the caller says "starting". */
export function startsIn(seconds: number): string | null {
  const minutes = Math.round((Number.isFinite(seconds) ? seconds : 0) / 60);
  return minutes < 1 ? null : duration(minutes * 60);
}

/** A span that is usually small but can run to minutes: `places` decimals
 *  under a minute, where the fraction still means something, and words
 *  ("10 min 40 s") from there up. */
export function span(s: number | null | undefined, places = 1): string {
  if (s === null || s === undefined || !Number.isFinite(s)) return "—";
  return s < 59.5 ? `${s.toFixed(places)}s` : duration(s);
}

/** "Live" at the edge, "-1m05s" behind it.
 *
 *  Takes the backend's `atLiveEdge`, not a threshold of its own. Deciding
 *  from `behindLive` here flickered between "-0:02" and "LIVE" as the
 *  seekable end sawtoothed, which the controls bar had already stopped doing
 *  by using the backend's hysteresis — and since live now sits a few seconds
 *  back on purpose, a bare threshold would never say "Live" at all. */
export function liveOffset(atLiveEdge: boolean, behindLive: number): string {
  return atLiveEdge ? "Live" : `-${duration(behindLive)}`;
}

/** The gap, said the way a person would: "4s", "3m05s". */
export function behindGap(behindLive: number): string {
  return duration(behindLive);
}

export interface LiveState {
  /** Red dot, or grey. */
  live: boolean;
  /** What the control says out loud: its title and its accessible name.
   *  Colour alone is not a message — a red dot and a grey one are the same dot
   *  to plenty of people. */
  label: string;
  /** Whether there is anywhere to jump to. At the edge there is not, and the
   *  control goes inert rather than pretending otherwise. */
  canJump: boolean;
}

/** What the live indicator shows.
 *
 *  `atLiveEdge` comes from the backend and is the only input to liveness.
 *  `behindLive` is measured against the end of mpv's seekable window, which
 *  advances in chunks as the demuxer reads, so it sawtooths by a couple of
 *  seconds during perfectly ordinary playback: any threshold taken here would
 *  sit inside that sawtooth and flip the indicator on every tick. The backend
 *  applies hysteresis — 3s to become live, 8s to stop being live — and the gap
 *  between those two numbers is the whole reason the dot holds still.
 *
 *  So `behindLive` is displayed, never thresholded. */
export function liveState(atLiveEdge: boolean, behindLive: number): LiveState {
  if (atLiveEdge) return { live: true, label: "Live", canJump: false };
  return { live: false, label: `Behind live by ${behindGap(behindLive)}`, canJump: true };
}

// ------------------------------------------------------------ times of day
//
// Every time of day the app shows goes through the helpers below, so the
// 12/24-hour setting reaches all of them at once. The setting lives in the
// store; `setTimeFormat` copies it here (StoreProvider does, as it renders,
// so the children rendering after it already read the new value). Each
// helper also takes the choice explicitly, for the checks and for memoised
// rows, which need it as a prop to know when to redraw.

/** "auto" follows the system's locale; the other two override it. */
export type TimeFormat = "auto" | "12h" | "24h";
export const TIME_FORMATS: readonly TimeFormat[] = ["auto", "12h", "24h"];

/** Whether the system's locale writes the time with am/pm. Asked with an
 *  hour in the options: without one, `resolvedOptions()` leaves `hourCycle`
 *  out altogether. */
export function systemUses12h(locale?: string): boolean {
  try {
    const cycle = new Intl.DateTimeFormat(locale, { hour: "numeric" }).resolvedOptions().hourCycle;
    return cycle === "h11" || cycle === "h12";
  } catch {
    return false;
  }
}

/** The setting's answer: an override, or the system's. Anything unknown (a
 *  hand-edited config) counts as "auto". */
export function uses12h(format: string, system: boolean = systemUses12h()): boolean {
  return format === "12h" ? true : format === "24h" ? false : system;
}

let twelve = systemUses12h();

/** Points the helpers below at a setting. Cheap, so whatever owns the
 *  setting can call it on every render. */
export function setTimeFormat(format: string): void {
  twelve = uses12h(format);
}

/** What the helpers use when they are not told: the setting, as last set. */
export const clockIs12h = (): boolean => twelve;

const pad = (n: number) => String(n).padStart(2, "0");

/** The hour and minute of an instant, in the local zone. */
function hm(unixSeconds: number): { h: number; m: number } {
  const d = new Date(unixSeconds * 1000);
  return { h: d.getHours(), m: d.getMinutes() };
}

const meridiem = (h: number) => (h < 12 ? "am" : "pm");
const hourOf12 = (h: number) => (h % 12 === 0 ? 12 : h % 12);

/** A time of day, the way the setting writes it: "20:05", or "8:05 pm".
 *  The 24-hour form keeps its leading zero ("08:05") so a column of them
 *  lines up; the 12-hour one drops it, as people write it. */
export function hhmm(unixSeconds: number, h12: boolean = twelve): string {
  const { h, m } = hm(unixSeconds);
  return h12 ? `${hourOf12(h)}:${pad(m)} ${meridiem(h)}` : `${pad(h)}:${pad(m)}`;
}

/** From one time to another: "20:00–21:30". On the 12-hour clock the am/pm
 *  is said once when both ends share it ("8:00–9:30 pm"), and at each end
 *  when they do not ("11:30 pm–12:30 am"). */
export function timeRange(from: number, to: number, h12: boolean = twelve): string {
  if (!h12) return `${hhmm(from, false)}–${hhmm(to, false)}`;
  const a = hm(from);
  if (meridiem(a.h) === meridiem(hm(to).h)) return `${hourOf12(a.h)}:${pad(a.m)}–${hhmm(to, true)}`;
  return `${hhmm(from, true)}–${hhmm(to, true)}`;
}

/** A mark on a time axis, where width is short. On the 12-hour clock the
 *  hour is "8 pm" and anything between is "8:30": the hour beside it has
 *  already said which half of the day. The 24-hour form is short already. */
export function timeTick(unixSeconds: number, h12: boolean = twelve): string {
  if (!h12) return hhmm(unixSeconds, false);
  const { h, m } = hm(unixSeconds);
  return m === 0 ? `${hourOf12(h)} ${meridiem(h)}` : `${hourOf12(h)}:${pad(m)}`;
}

/** A date and a time, for placing a moment in the past ("Last synced"): the
 *  locale's short date, then the time as the setting writes it. */
/** A day, as people write one: "1 Mar 2027" in the locale's order. For an
 *  account's expiry, where the time of day says nothing. */
export function calendarDate(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

export function dateTime(unixSeconds: number, h12: boolean = twelve): string {
  const date = new Date(unixSeconds * 1000).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
  return `${date}, ${hhmm(unixSeconds, h12)}`;
}

/** How far through a programme we are, 0-1, for the row's progress hairline. */
export function programmeProgress(
  start: number,
  stop: number,
  nowSeconds: number,
): number {
  if (!(stop > start)) return 0;
  const t = (nowSeconds - start) / (stop - start);
  return Math.min(1, Math.max(0, t));
}

/** Volume can exceed 100 (mpv's volume-max), so the readout must not clamp at
 *  100 — it clamps at the ceiling the caller passes. */
export function clampVolume(v: number, max: number): number {
  return Math.min(max, Math.max(0, Math.round(v)));
}

/** The badge for whatever is playing, from what the player actually measured.
 *  Same rule as the row badge — height decides, a name proves nothing — and
 *  nothing measured means no badge at all. */
export function playerBadge(p: PlayerStateDto): string | null {
  if (p.width == null && p.height == null) return null;
  return qualityBadge({ height: p.height, isHdr: false });
}

/** The two captions under the scrubber: what the left of the bar is, and
 *  what the right of it is.
 *
 *  `compact` is the docked picture's bar, a fraction of the player's width:
 *  the same two facts in the fewest words that still say them — how far back
 *  the archive goes, and how much is held locally. */
export function scrubLabels(o: {
  hasArchive: boolean;
  archiveDays: number | null;
  bufferedBack: number;
  hasChannel: boolean;
  compact: boolean;
}): [string, string] {
  const held = duration(o.bufferedBack);
  if (o.hasArchive) {
    return o.compact
      ? [`${o.archiveDays}d archive`, `${held} buffer`]
      : [`${o.archiveDays}-day archive · seek reconnects`, `buffer ${held} · instant`];
  }
  if (o.compact) return [o.hasChannel ? "no catchup" : "local buffer", `${held} back`];
  return [
    `local buffer only${o.hasChannel ? " — no catchup on this channel" : ""}`,
    `${held} of scrollback`,
  ];
}

/** "52m", "1h 49m", "2h 01m"; nothing for an unknown or zero length. */
export function runtime(secs: number | null): string {
  if (!secs || secs <= 0) return "";
  const minutes = Math.round(secs / 60);
  if (minutes < 60) return `${minutes}m`;
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, "0")}m`;
}
