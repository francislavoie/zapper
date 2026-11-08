// A movie or an episode in a browser signed in to the recording service:
// what the app's player does for a title (play from a place, seek, pause,
// another sound, subtitles, report how far it got), made of the page's
// <video> element. Both are watched the same way; only the title named in
// the watch, and where its place is reported, differ.
//
// The service decides how the film reaches this browser
// (recording::vodwatch, `POST /api/vod/watch`). A file the browser plays as
// it is comes "direct": the element reads it by byte ranges and seeks it
// itself. Anything else is remuxed to MPEG-TS from a time ("remux"), which
// mpegts.js hands to Media Source Extensions; seeking past what the element
// holds asks for a new remux from there. The remux's answer names the
// keyframe it really starts on (`X-Start`), which may be before the time
// asked for: the film's time is that start plus the element's clock, so the
// remux is read through a loader of ours that sees the answer's headers.
//
// The film plays on one of the account's connections, as a channel does,
// under the same rules: refused on a full account with "Watch here anyway"
// when that is the way, warned about a recording that needs the connection.
// A film paused for a minute lets the connection go, its place kept, and
// opens again there when played.
//
// Text subtitles come from the same read of the file: the service keeps
// what the current remux has decoded, and the page asks for a chosen
// track's newer cues every couple of seconds, put on a text track of the
// element. Subtitles that are pictures are listed and not shown.
//
// The pure half is src/logic/webVod.ts.

import type MpegtsType from "mpegts.js";
import { requestInit } from "./logic/web";
import {
  UNSUPPORTED,
  browserCan,
  endingOf,
  meansAnyway,
  refusalState,
  type MseHost,
  type WatchNotice,
  type WatchRefusal,
} from "./logic/webPlayback";
import {
  NOT_STUCK,
  SUBTITLE_POLL_MS,
  bufferedEnd,
  carriedIndex,
  cueTimes,
  movedAt,
  needsData,
  openDue,
  parseVtt,
  positionOf,
  progressDue,
  progressPath,
  releaseDue,
  reopenDue,
  seekPlan,
  stallDue,
  stuckAfterPlay,
  stuckAgain,
  stuckTooOften,
  subtitleAfter,
  upNextAllowed,
  vodStreamUrl,
  watchBody,
  type Stuck,
  type VodItemRef,
  type VodMode,
} from "./logic/webVod";
import type { OnDemandDto, PlayerOptionsDto, StreamEnding, TrackDto } from "./types";
import {
  currentAspect,
  currentVideo,
  loadMpegts,
  noticeFeed,
  optionsFeed,
  patch,
  playableHere,
  playerFeed,
  signedOutNow,
  stop as stopPlayer,
  takeLive,
} from "./webPlayer";

type Mpegts = typeof MpegtsType;
type Player = ReturnType<Mpegts["createPlayer"]>;
type LoaderError = Parameters<InstanceType<Mpegts["BaseLoader"]>["onError"]>[0];

const host = globalThis as unknown as MseHost;

// ----------------------------------------------------------------- state

interface Track {
  index: number;
  codec: string;
  lang: string | null;
  described: boolean;
  impaired: boolean;
}

interface Subtitle {
  index: number;
  language: string | null;
  title: string | null;
  /** Text, which the page shows; pictures it cannot. */
  text: boolean;
}

interface VodAnswer {
  viewer: string;
  stream: string;
  mode: VodMode;
  duration: number | null;
  audio: Track[];
  audioIndex: number | null;
  subtitles: Subtitle[];
  converting: boolean;
  notice: WatchNotice | null;
}

interface Session {
  item: VodItemRef;
  viewer: string;
  stream: string;
  mode: VodMode;
  duration: number | null;
  tracks: Track[];
  audioIndex: number | null;
  subtitles: Subtitle[];
  /** The subtitle track shown: its place in `subtitles`. */
  subtitle: number | null;
  /** The current reading of the stream, numbered so that a late event from
   *  an older one is ignored. */
  reading: number;
  /** Where it was asked to start, and (remux) where it really starts once
   *  the answer says: the element's zero, in the film's time. */
  from: number;
  start: number | null;
  player: Player | null;
  response: Reading | null;
  /** The element's listeners for this reading. */
  events: AbortController | null;
  /** The picture has been started on this reading. */
  started: boolean;
  /** Played once ready; a reading opened while paused stays paused. */
  wantPlay: boolean;
  openedAt: number;
  /** Where in the film the picture is, in seconds. */
  position: number;
  pausedAt: number | null;
  /** Let go after a long pause; playing again watches anew. */
  released: boolean;
  lastProgress: { at: number; position: number } | null;
  /** Reopened after the stream broke, within the last minute. */
  reopens: number[];
  /** When the playing film's place last moved (logic/webVod.ts `movedAt`). */
  moved: { at: number; position: number } | null;
  /** Opened again in a row for not starting or standing still, with not
   *  enough playing in between to say it got over it. */
  stuck: Stuck;
  /** The subtitle cues put on the element's track for this reading. */
  cues: { lastEnd: number | null; held: Set<string>; askedAt: number; asking: boolean };
}

let session: Session | null = null;
let seq = 0;
/** The last title that failed, and how: playing the same one again from
 *  the picture's "Watch here anyway" means exactly that. */
let lastFailure: { item: VodItemRef; ending: StreamEnding | null } | null = null;
let ticker: ReturnType<typeof setInterval> | undefined;
let poller: ReturnType<typeof setInterval> | undefined;
/** The title that has just played to its end, which Up next may move on
 *  from, with the languages it was heard and read in; cleared by any watch
 *  or stop. The stopped state says so as `finished`. */
let finished: VodItemRef | null = null;
let finishedLanguages: Languages | null = null;

/** The sound's and the subtitles' languages, carried to the next episode. */
interface Languages {
  audio: string | null;
  subtitle: string | null;
}

function languagesOf(s: Session): Languages {
  const audio = s.audioIndex === null ? null : (s.tracks[s.audioIndex]?.lang ?? null);
  const subtitle = s.subtitle === null ? null : (s.subtitles[s.subtitle]?.language ?? null);
  return { audio, subtitle };
}

/** Whether a title is open (playing, paused or opening). */
export const active = (): boolean => session !== null;

const sameItem = (a: VodItemRef, b: VodItemRef): boolean => a.kind === b.kind && a.id === b.id;

/** What the title is called in what the picture says about it. */
const noun = (item: VodItemRef): string => (item.kind === "episode" ? "episode" : "movie");

function onDemandOf(
  item: VodItemRef,
  position: number,
  duration: number | null,
  bufferedTo: number | null,
): OnDemandDto {
  return {
    kind: item.kind,
    id: item.id,
    // Whole seconds, as the app's player reports them: the readout moves
    // once a second.
    position: Math.max(0, Math.floor(position)),
    duration: duration !== null && duration > 0 ? Math.round(duration) : null,
    bufferedTo: bufferedTo === null ? null : Math.floor(bufferedTo),
  };
}

/** Reports the film's place to the player's listeners, when it moved. */
function showPlace(s: Session, bufferedTo: number | null) {
  const next = onDemandOf(s.item, s.position, durationOf(s), bufferedTo);
  const was = playerFeed.last.onDemand;
  if (
    was &&
    was.kind === next.kind &&
    was.id === next.id &&
    was.position === next.position &&
    was.duration === next.duration &&
    was.bufferedTo === next.bufferedTo
  ) {
    return;
  }
  patch({ onDemand: next });
}

/** The film's length: a direct file's own once the element knows it (the
 *  provider's details may be off by minutes), else what the service said. */
function durationOf(s: Session): number | null {
  const d = currentVideo()?.duration;
  if (s.mode === "direct" && d !== undefined && Number.isFinite(d) && d > 0) return d;
  return s.duration;
}

function ranges(el: HTMLVideoElement): [number, number][] {
  const out: [number, number][] = [];
  const b = el.buffered;
  for (let i = 0; i < b.length; i++) out.push([b.start(i), b.end(i)]);
  return out;
}

// ------------------------------------------------------------- watching

async function watchRequest(
  item: VodItemRef,
  anyway: boolean,
  replaces: string | null,
  audio: number | null,
): Promise<{ status: number; body: unknown }> {
  try {
    const resp = await fetch(
      "/api/vod/watch",
      requestInit("POST", watchBody(item, { anyway, replaces, audio, can: browserCan(host) })),
    );
    const body: unknown = await resp.json().catch(() => null);
    return { status: resp.status, body };
  } catch {
    return { status: 0, body: null };
  }
}

/** Plays movie `movieId` from `start` seconds in (from its beginning with
 *  `null`). Resolves once the service has answered; what happens then comes
 *  through `playerFeed`, as with a channel. */
export function playMovie(movieId: number, start: number | null): Promise<void> {
  return playItem({ kind: "movie", id: movieId }, start);
}

/** Plays episode `episodeId` from `start` seconds in, as `playMovie` does a
 *  movie. */
export function playEpisode(episodeId: number, start: number | null): Promise<void> {
  return playItem({ kind: "episode", id: episodeId }, start);
}

function playItem(item: VodItemRef, start: number | null): Promise<void> {
  return watch(item, Math.max(0, start ?? 0), {
    audio: null,
    subtitle: null,
    duration: null,
    wantPlay: true,
  });
}

interface Keep {
  audio: number | null;
  subtitle: number | null;
  duration: number | null;
  wantPlay: boolean;
  /** Another title's languages, whose tracks are chosen once this one's
   *  are known (Up next), in place of `audio` and `subtitle`. */
  carry?: Languages;
}

/** Asks the service for the film, from `from`, keeping what was chosen on
 *  an earlier watch of it (`keep`). */
async function watch(item: VodItemRef, from: number, keep: Keep): Promise<void> {
  if (!playableHere) throw new Error(UNSUPPORTED);
  const anyway =
    lastFailure !== null && sameItem(lastFailure.item, item) && meansAnyway(lastFailure.ending);
  lastFailure = null;
  finished = null;
  finishedLanguages = null;
  // What was on goes first; the service is told to let its viewer go as
  // this one is made, so a single connection serves the next.
  const replaces = leave() ?? takeLive();
  const my = ++seq;
  noticeFeed.set({ recording: null, silent: null });
  optionsFeed.set({ audio: [], subtitles: [], aspect: currentAspect(), deinterlace: "auto" });
  patch({
    state: "opening",
    streamId: null,
    width: null,
    height: null,
    fps: null,
    codec: null,
    bufferedBack: 0,
    behindLive: 0,
    atLiveEdge: true,
    error: null,
    liveJump: null,
    ending: null,
    reconnect: null,
    joiningLive: false,
    onDemand: onDemandOf(item, from, keep.duration, null),
    finished: null,
  });
  const { status, body } = await watchRequest(item, anyway, replaces, keep.audio);
  if (my !== seq) {
    // Superseded while asking: let the answer's viewer go at once.
    const late = body as Partial<VodAnswer> | null;
    if (status === 200 && late?.viewer) void forget(late.viewer);
    return;
  }
  if (status !== 200) {
    if (status === 401) signedOutNow();
    const r = refusalState(status, body as WatchRefusal | null);
    fail(item, from, keep.duration, r.ending, r.error);
    return;
  }
  const a = body as VodAnswer;
  const subtitles = a.subtitles ?? [];
  // Up next: the sound and subtitles in the languages of the episode before
  // (only text subtitles can be shown).
  const carried = keep.carry
    ? {
        audio: carriedIndex(keep.carry.audio, (a.audio ?? []).map((t) => ({ lang: t.lang }))),
        subtitle: carriedIndex(
          keep.carry.subtitle,
          subtitles.map((t) => ({ lang: t.text ? t.language : null })),
        ),
      }
    : null;
  const subtitle = carried ? carried.subtitle : keep.subtitle;
  const s: Session = {
    item,
    viewer: a.viewer,
    stream: a.stream,
    mode: a.mode === "direct" ? "direct" : "remux",
    duration: a.duration ?? keep.duration,
    tracks: a.audio ?? [],
    audioIndex: a.audioIndex ?? null,
    subtitles,
    subtitle: subtitle !== null && subtitles[subtitle]?.text ? subtitle : null,
    reading: 0,
    from,
    start: null,
    player: null,
    response: null,
    events: null,
    started: false,
    wantPlay: keep.wantPlay,
    openedAt: Date.now(),
    position: from,
    pausedAt: null,
    released: false,
    lastProgress: null,
    reopens: [],
    moved: null,
    stuck: NOT_STUCK,
    cues: { lastEnd: null, held: new Set(), askedAt: 0, asking: false },
  };
  session = s;
  if (carried?.audio != null && carried.audio !== s.audioIndex) {
    if (s.mode === "direct") {
      // A direct file plays its first sound: another is a remux, watched in
      // this one's place (as choosing it from the menu does).
      await watch(item, from, {
        audio: carried.audio,
        subtitle: s.subtitle,
        duration: s.duration,
        wantPlay: keep.wantPlay,
      });
      return;
    }
    // A remux reads the sound it is asked for.
    s.audioIndex = carried.audio;
  }
  optionsFeed.set(optionsOf(s));
  noticeFeed.set({ recording: a.notice ?? null, silent: null });
  showPlace(s, null);
  if (s.mode === "remux") {
    try {
      await loadMpegts();
    } catch {
      if (session === s) {
        fail(item, from, s.duration, null, "The player couldn't be loaded. Reload the page.");
      }
      return;
    }
    if (session !== s) return;
  }
  await openReading(s, from, keep.wantPlay);
  if (session === s) startTimers();
}

/** Reads the session's stream into the element from `from` seconds into
 *  the film: the file itself, sought there, or a remux from there. */
async function openReading(s: Session, from: number, wantPlay: boolean) {
  const el = currentVideo();
  if (!el) return;
  closeReading(s);
  const reading = ++s.reading;
  s.from = from;
  s.start = s.mode === "direct" ? 0 : null;
  s.position = from;
  s.started = false;
  s.wantPlay = wantPlay;
  s.openedAt = Date.now();
  s.pausedAt = null;
  s.moved = null;
  resetCues(s);
  patch({ state: "opening" });
  const current = () => session === s && s.reading === reading && !s.released;

  const events = new AbortController();
  s.events = events;
  const on = (type: string, f: () => void, once = false) =>
    el.addEventListener(type, f, { signal: events.signal, once });
  on("playing", () => {
    if (!current()) return;
    s.pausedAt = null;
    patch({ state: "playing" });
  });
  on("pause", () => {
    if (!current() || !s.started) return;
    s.pausedAt = Date.now();
    patch({ state: "paused" });
  });
  on("ended", () => {
    if (current()) void ended(s);
  });
  on("error", () => {
    if (current()) void broke(s, reading);
  });

  if (s.mode === "direct") {
    // The browser's own decoding: nothing of a remux's to say.
    delete el.dataset.audioCodec;
    delete el.dataset.videoCodec;
    el.preload = "auto";
    if (from > 0) {
      on("loadedmetadata", () => {
        if (current()) el.currentTime = from;
      }, true);
    }
    el.src = new URL(s.stream, location.href).href;
    return;
  }

  const mpegts = await loadMpegts();
  if (!current()) return;
  const url = new URL(vodStreamUrl(s.stream, from, s.audioIndex), location.href).href;
  const response = new Reading(url, (at) => {
    if (!current()) return;
    s.start = at;
    // The picture's place until it moves: where the remux really starts.
    if (!s.started) s.position = at;
  });
  s.response = response;
  const silent = s.tracks.length === 0 && s.audioIndex === null;
  const p = mpegts.createPlayer(
    {
      type: "mpegts",
      isLive: false,
      url: response.url,
      ...(silent ? { hasAudio: false } : {}),
    },
    {
      enableWorker: false,
      customLoader: loaderFor(mpegts, response),
      // Read ahead a minute, then wait: the remux is paced by the page, and
      // the account's connection is not pulled faster than the film plays.
      lazyLoad: true,
      lazyLoadMaxDuration: 60,
      lazyLoadRecoverDuration: 20,
      autoCleanupSourceBuffer: true,
      autoCleanupMaxBackwardDuration: 120,
      autoCleanupMinBackwardDuration: 60,
    },
  );
  p.on(mpegts.Events.ERROR, () => {
    if (current()) void broke(s, reading);
  });
  p.on(mpegts.Events.MEDIA_INFO, (mi: { videoCodec?: string; audioCodec?: string; fps?: number }) => {
    if (!current()) return;
    patch({ codec: mi.videoCodec ?? null, fps: mi.fps ?? null });
    el.dataset.audioCodec = mi.audioCodec ?? "";
    el.dataset.videoCodec = mi.videoCodec ?? "";
  });
  s.player = p;
  p.attachMediaElement(el);
  p.load();
}

/** Lets go of the session's reading: its player, its response and its
 *  listeners. The viewer stays. */
function closeReading(s: Session) {
  s.events?.abort();
  s.events = null;
  s.response?.cancel();
  s.response = null;
  if (s.player) {
    try {
      s.player.destroy();
    } catch {
      // Already torn down.
    }
    s.player = null;
  }
  const el = currentVideo();
  if (el && s.mode === "direct" && el.hasAttribute("src")) {
    el.removeAttribute("src");
    el.load();
  }
}

/** Starts the picture once the element has enough of it. */
function startWhenReady(s: Session, el: HTMLVideoElement) {
  const ready = el.readyState >= 3 && !el.seeking;
  const waited = Date.now() - s.openedAt;
  const ahead = (bufferedEnd(ranges(el), el.currentTime, 0) ?? 0) - el.currentTime;
  if (!ready && !(waited > 8000 && ahead > 0.5)) return;
  s.started = true;
  if (!s.wantPlay) {
    s.pausedAt = Date.now();
    patch({ state: "paused" });
    return;
  }
  el.play()?.catch(() => {
    // Refused without a click behind it (a page reloaded into playing): the
    // picture waits for Play.
    if (session !== s) return;
    s.pausedAt = Date.now();
    patch({ state: "paused" });
  });
}

/** The element got to the end of what it had: the end of the film, or a
 *  remux that stopped short of it. A direct file's end is the film's: the
 *  element knows the file's length. */
async function ended(s: Session) {
  const d = durationOf(s);
  if (s.mode === "direct" || d === null || s.position >= d - 10) {
    s.position = d ?? s.position;
    const languages = languagesOf(s);
    await stop();
    await stopPlayer();
    // Finished, not stopped: Up next may move on from it. Unless something
    // was watched meanwhile.
    if (session === null && playerFeed.last.state === "stopped") {
      finished = s.item;
      finishedLanguages = languages;
      patch({ finished: onDemandOf(s.item, s.position, d, null) });
    }
    return;
  }
  await broke(s, s.reading);
}

/** The stream broke: ask the service why. When it says the film is fine,
 *  it is opened again where it was (a few times a minute at most);
 *  otherwise the picture says what happened. */
async function broke(s: Session, reading: number) {
  if (session !== s || s.released || s.reading !== reading) return;
  const refused = s.response?.refused ?? null;
  // A 409 (an older reading would not let the file go in time) is asked
  // about and opened again, as any other break.
  if (refused) {
    if (refused.status === 401) signedOutNow();
    if ([400, 401, 404, 422].includes(refused.status)) {
      const r = refusalState(refused.status, refused.body as WatchRefusal | null);
      fail(s.item, s.position, s.duration, r.ending, r.error);
      return;
    }
  }
  const st = await viewerStatus(s.viewer);
  if (session !== s || s.released || s.reading !== reading) return;
  if (st?.gone) {
    await rewatch(s);
    return;
  }
  if (st?.ended) {
    if (st.takenOver) {
      fail(s.item, s.position, s.duration, endedBy("inUse"),
        "Another browser started watching on this account's connection.");
    } else {
      fail(s.item, s.position, s.duration, endingOf(st.ended), `The ${noun(s.item)} stopped.`);
    }
    return;
  }
  const now = Date.now();
  const due = reopenDue(s.reopens, now);
  s.reopens = due.keep;
  if (st === null || due.fail) {
    fail(s.item, s.position, s.duration, endedBy("network"),
      `The ${noun(s.item)} from the recording service broke off.`);
    return;
  }
  s.reopens.push(now);
  const el = currentVideo();
  // An element that ran out of the stream (`ended`) says it is paused, but
  // was playing: it plays on when opened again, and a stream that keeps
  // stopping short ends in the failure above rather than a quiet pause.
  await openReading(s, s.position, el ? !el.paused || el.ended || !s.started : true);
}

const endedBy = (kind: StreamEnding["kind"]): StreamEnding => ({
  kind,
  slots: null,
  status: null,
  limit: false,
  clipSecs: null,
});

/** The film stopped for a reason, shown over the picture, its place kept
 *  for Retry. */
function fail(
  item: VodItemRef,
  position: number,
  duration: number | null,
  ending: StreamEnding | null,
  error: string,
) {
  const s = session;
  session = null;
  if (s) {
    report(s, true);
    closeReading(s);
    clearSubtitles();
    if (!s.released) void forget(s.viewer);
  }
  lastFailure = { item, ending };
  stopTimers();
  noticeFeed.set({ recording: null, silent: null });
  patch({
    state: "failed",
    streamId: null,
    ending,
    error,
    reconnect: null,
    joiningLive: false,
    onDemand: onDemandOf(item, position, duration, null),
    finished: null,
  });
}

/** Gives the open film up: its place reported, its reading let go. Its
 *  viewer is returned for whatever is watched next to replace, or `null`
 *  when there is none to. */
function leave(): string | null {
  seq++;
  const s = session;
  session = null;
  stopTimers();
  clearSubtitles();
  if (!s) return null;
  report(s, true);
  closeReading(s);
  return s.released ? null : s.viewer;
}

/** For a channel about to play in its place: the film's viewer, for the
 *  channel's watch to replace. */
export function handOver(): string | null {
  lastFailure = null;
  finished = null;
  finishedLanguages = null;
  return leave();
}

/** Stops the film, its place kept. The player's own `stop` (webPlayer)
 *  clears the picture after. */
export async function stop(): Promise<void> {
  lastFailure = null;
  finished = null;
  finishedLanguages = null;
  const viewer = leave();
  if (viewer) await forget(viewer);
}

/** Up next: moves on from episode `from`, playing or just played to its
 *  end, to episode `next` from its start, in the languages `from` was in.
 *  `from` is marked watched first. The next one's viewer replaces the old
 *  one's, so one connection serves both. It opens paused when `from` was
 *  paused, as the app's player does; the action that asked resumes it.
 *  Refused unless `from` is still what is open or what finished. */
export async function playNextEpisode(from: number, next: number): Promise<void> {
  if (from === next) throw new Error("That is the episode playing.");
  const s = session;
  const subject = () =>
    session
      ? { item: session.item, state: playerFeed.last.state, finished: false }
      : finished
        ? { item: finished, state: playerFeed.last.state, finished: true }
        : null;
  const refused = new Error("That episode is no longer the one playing.");
  if (!upNextAllowed(subject(), from)) throw refused;
  const carry = s ? languagesOf(s) : (finishedLanguages ?? { audio: null, subtitle: null });
  const paused = s !== null && playerFeed.last.state === "paused";
  try {
    const r = await fetch(`/api/vod/episodes/${from}/marks`, requestInit("PUT", { watched: true }));
    if (r.status === 401) signedOutNow();
    else if (!r.ok) {
      // Not worth holding the next one for: its place, reported as it
      // played, still says how far it got.
      console.warn(`Up next: the episode moved on from was not marked watched (${r.status})`);
    }
  } catch {
    console.warn("Up next: the episode moved on from could not be marked watched");
  }
  // Stopped, or something else played, while it was being marked.
  if (session !== s || !upNextAllowed(subject(), from)) throw refused;
  await watch({ kind: "episode", id: next }, 0, {
    audio: null,
    subtitle: null,
    duration: null,
    wantPlay: !paused,
    carry,
  });
}

/** Lets the service's viewer go. `keepalive`: sent even as the page goes. */
async function forget(viewer: string, keepalive = false) {
  try {
    await fetch(`/api/vod/watch/${encodeURIComponent(viewer)}`, {
      ...requestInit("DELETE"),
      keepalive,
    });
  } catch {
    // The service lets an unread viewer go by itself.
  }
}

interface ViewerStatus {
  ended: { kind: string; status: number | null; limit: boolean } | null;
  takenOver: boolean;
  notice: WatchNotice | null;
  /** The service has no such viewer (any more): it let it go, unread. */
  gone?: boolean;
}

/** When viewers were made again by `rewatch`, the last minute's. */
let rewatches: number[] = [];

/** The service let the film's viewer go (nothing read, and its page not
 *  heard from for a while: a timer held back in a hidden tab). Nothing has
 *  failed, so it is quietly watched again from where it is, a few times a
 *  minute at most. */
async function rewatch(s: Session) {
  const now = Date.now();
  rewatches = rewatches.filter((t) => now - t < 60_000);
  if (rewatches.length >= 3) {
    fail(s.item, s.position, s.duration, endedBy("network"),
      `The recording service let the ${noun(s.item)} go.`);
    return;
  }
  rewatches.push(now);
  const el = currentVideo();
  // Its viewer is gone: there is nothing to let go or replace.
  s.released = true;
  await watch(s.item, s.position, {
    audio: s.audioIndex,
    subtitle: s.subtitle,
    duration: s.duration,
    wantPlay: el ? !el.paused || !s.started : true,
  });
}

async function viewerStatus(viewer: string): Promise<ViewerStatus | null> {
  try {
    const r = await fetch(`/api/vod/watch/${encodeURIComponent(viewer)}`, requestInit("GET"));
    if (r.status === 401) signedOutNow();
    if (r.status === 404 || r.status === 410) {
      return { ended: null, takenOver: false, notice: null, gone: true };
    }
    if (!r.ok) return null;
    return (await r.json()) as ViewerStatus;
  } catch {
    return null;
  }
}

/** Tells the service how far the film got: it keeps the place to resume
 *  from, and past most of the film marks it watched. `final`: whatever the
 *  timing, as the film is left; `keepalive`: sent even as the page goes. */
function report(s: Session, final = false, keepalive = false) {
  if (!final && !progressDue(s.lastProgress, Date.now(), s.position, s.pausedAt !== null)) return;
  if (s.position < 1 && s.lastProgress === null) return;
  if (final && s.lastProgress?.position === s.position) return;
  s.lastProgress = { at: Date.now(), position: s.position };
  const body = { position: s.position, duration: durationOf(s) };
  void fetch(progressPath(s.item), {
    ...requestInit("PUT", body),
    keepalive,
  }).catch(() => {
    // The next report says it again.
  });
}

// ------------------------------------------------------------- timers

function startTimers() {
  stopTimers();
  ticker = setInterval(tick, 250);
  poller = setInterval(() => void poll(), 5000);
}

function stopTimers() {
  if (ticker !== undefined) clearInterval(ticker);
  if (poller !== undefined) clearInterval(poller);
  ticker = poller = undefined;
}

/** Where the picture is; the reports, the long pause, the subtitles. */
function tick() {
  const s = session;
  const el = currentVideo();
  if (!s || !el || s.released) return;
  const now = Date.now();
  if (!s.started) startWhenReady(s, el);
  const base = s.start ?? s.from;
  if (s.started) s.position = positionOf(s.mode, base, el.currentTime);
  showPlace(s, bufferedEnd(ranges(el), el.currentTime, s.mode === "remux" ? base : 0));
  if (!s.started) {
    if (openDue(s.openedAt, now)) stuck(s);
    return;
  }
  const moved = movedAt(s.moved, now, s.position, el.paused);
  // Playing again after being stuck: the count starts afresh once it has
  // played a while past where it stuck.
  if (moved !== s.moved && s.moved !== null && !el.paused) {
    s.stuck = stuckAfterPlay(s.stuck, s.moved.position, moved.position);
  }
  s.moved = moved;
  if (stallDue(moved, now)) {
    stuck(s);
    return;
  }
  report(s);
  if (s.pausedAt !== null && releaseDue(s.pausedAt, now)) {
    release(s);
    return;
  }
  if (s.pausedAt === null && now - s.cues.askedAt >= SUBTITLE_POLL_MS) void askCues(s);
}

/** The film would not start, or stood still while playing: asked about and
 *  opened again (`broke`), a few times in a row at most before the picture
 *  says it stopped, which lets its viewer go. */
function stuck(s: Session) {
  s.stuck = stuckAgain(s.stuck);
  if (stuckTooOften(s.stuck)) {
    fail(s.item, s.position, s.duration, endedBy("network"),
      `The ${noun(s.item)} stopped coming from the recording service.`);
    return;
  }
  // Not again until this has been dealt with: a new reading counts afresh.
  s.openedAt = Date.now();
  s.moved = null;
  void broke(s, s.reading);
}

/** Paused for long, or the page going: the connection goes, the place
 *  stays. Playing again watches the film anew from there. `leaving`: the
 *  page is being closed or put away, and what is sent must outlive it. */
function release(s: Session, leaving = false) {
  report(s, true, leaving);
  s.released = true;
  closeReading(s);
  void forget(s.viewer, leaving);
  noticeFeed.set({ recording: null, silent: null });
  patch({ state: "paused", bufferedBack: 0 });
}

// A closed tab would otherwise hold the film's connection until the service
// gives up on a page that stopped asking after it, which a recording due
// meanwhile waits for. Let go as the page goes; a page restored from the
// browser's back-forward cache watches again when played, as after a long
// pause.
if (typeof window !== "undefined") {
  window.addEventListener("pagehide", () => {
    const s = session;
    if (s && !s.released) release(s, true);
  });
}

/** What the service says about this viewer: a recording it is in the way
 *  of, or that it ended. A page nobody looks at gives the connection up as
 *  soon as a recording waits for it. */
async function poll() {
  const s = session;
  if (!s || s.released) return;
  const reading = s.reading;
  const st = await viewerStatus(s.viewer);
  if (session !== s || s.released || !st) return;
  if (st.gone) {
    // Let go while the browser played from what it holds (a hidden tab's
    // timers, held back): watched again only once it wants more; the
    // element's next request, refused, says so too.
    const el = currentVideo();
    if (!el || needsData(el.readyState)) await rewatch(s);
    return;
  }
  if (st.ended) {
    void broke(s, reading);
    return;
  }
  noticeFeed.set({ recording: st.notice, silent: null });
  if (st.notice?.waiting && typeof document !== "undefined" && document.hidden) {
    release(s);
  }
}

// ------------------------------------------------------------- controls

export async function togglePause(): Promise<void> {
  const s = session;
  const el = currentVideo();
  if (!s || !el) return;
  if (s.released) {
    await watch(s.item, s.position, {
      audio: s.audioIndex,
      subtitle: s.subtitle,
      duration: s.duration,
      wantPlay: true,
    });
    return;
  }
  if (!s.started) {
    s.wantPlay = !s.wantPlay;
    return;
  }
  if (el.paused) await el.play().catch(() => {});
  else el.pause();
}

/** Goes to `seconds` into the film: within what the element holds, or by
 *  reading it again from there. */
export async function seekTo(seconds: number): Promise<void> {
  const s = session;
  const el = currentVideo();
  if (!s || !el || !Number.isFinite(seconds)) return;
  const d = durationOf(s);
  const target = Math.max(0, d !== null ? Math.min(d - 1, seconds) : seconds);
  if (s.released) {
    // Opened there when it plays again.
    s.position = target;
    showPlace(s, null);
    return;
  }
  const plan = s.started
    ? seekPlan(s.mode, target, s.start ?? s.from, ranges(el))
    : ({ kind: "reopen", from: target } as const);
  if (plan.kind === "native") {
    el.currentTime = plan.time;
    s.position = target;
    s.cues.lastEnd = null;
    showPlace(s, null);
    return;
  }
  await openReading(s, plan.from, s.started ? !el.paused : s.wantPlay);
}

export function seekRelative(by: number): Promise<void> {
  const s = session;
  if (!s) return Promise.resolve();
  return seekTo(s.position + by);
}

// ---------------------------------------------------------------- tracks

function optionsOf(s: Session): PlayerOptionsDto {
  const audio: TrackDto[] = s.tracks.map((t, i) => ({
    id: i + 1,
    title: null,
    lang: t.lang,
    codec: t.codec,
    channels: null,
    default: i === 0,
    forced: false,
    hearingImpaired: t.impaired,
    visualImpaired: t.described,
    selected: s.audioIndex === i,
  }));
  const subtitles: TrackDto[] = s.subtitles.map((t, i) => ({
    id: i + 1,
    title: t.title,
    lang: t.language,
    // What the menu needs to know of it: whether it is text, which is shown
    // here, or pictures, which are not.
    codec: t.text ? "webvtt" : "picture",
    channels: null,
    default: false,
    forced: false,
    hearingImpaired: false,
    visualImpaired: false,
    selected: s.subtitle === i,
  }));
  return { audio, subtitles, aspect: currentAspect(), deinterlace: "auto" };
}

/** Another sound: a remux reads the film again from where it is, with that
 *  one; a direct file is watched again as a remux, the only way to choose
 *  another sound than its first. */
export async function setAudioTrack(id: number): Promise<PlayerOptionsDto> {
  const s = session;
  const el = currentVideo();
  if (!s || !el) return optionsFeed.last;
  const index = id - 1;
  if (index === s.audioIndex || index < 0 || index >= s.tracks.length) return optionsFeed.last;
  const playing = s.started ? !el.paused : s.wantPlay;
  if (s.mode === "direct" || s.released) {
    await watch(s.item, s.position, {
      audio: index,
      subtitle: s.subtitle,
      duration: s.duration,
      wantPlay: playing || s.released,
    });
    return optionsFeed.last;
  }
  s.audioIndex = index;
  optionsFeed.set(optionsOf(s));
  await openReading(s, s.position, playing);
  return optionsFeed.last;
}

/** Shows subtitle track `id`, or none with `null`. Pictures can't be. */
export async function setSubtitleTrack(id: number | null): Promise<PlayerOptionsDto> {
  const s = session;
  if (!s) return optionsFeed.last;
  const index = id === null ? null : id - 1;
  if (index !== null && !s.subtitles[index]?.text) return optionsFeed.last;
  s.subtitle = index;
  resetCues(s);
  const o = optionsOf(s);
  optionsFeed.set(o);
  if (index !== null && s.started && s.pausedAt === null) void askCues(s);
  return o;
}

export const options = (): PlayerOptionsDto => optionsFeed.last;

// ------------------------------------------------------------- subtitles

let cueTrack: { el: HTMLVideoElement; track: TextTrack } | null = null;

/** The element's one text track for a film's subtitles: made once, its
 *  cues replaced as films, readings and choices change. */
function textTrack(el: HTMLVideoElement): TextTrack {
  if (cueTrack?.el !== el) cueTrack = { el, track: el.addTextTrack("subtitles", "Subtitles") };
  return cueTrack.track;
}

/** Empties the track; shown again when a subtitle is chosen. */
function clearSubtitles() {
  if (!cueTrack) return;
  const t = cueTrack.track;
  // A disabled track lists no cues: hidden, they can be taken off.
  t.mode = "hidden";
  const cues = t.cues;
  while (cues && cues.length > 0) t.removeCue(cues[0]);
  t.mode = "disabled";
}

/** A new reading, or another choice: the cues start afresh. */
function resetCues(s: Session) {
  clearSubtitles();
  s.cues = { lastEnd: null, held: new Set(), askedAt: 0, asking: false };
  const el = currentVideo();
  if (el && s.subtitle !== null) textTrack(el).mode = "showing";
}

/** Asks for the chosen track's cues that end after those held, and puts
 *  them on the element's clock. */
async function askCues(s: Session) {
  const index = s.subtitle;
  const el = currentVideo();
  if (index === null || !el || s.start === null || s.cues.asking) return;
  const reading = s.reading;
  const cues = s.cues;
  cues.asking = true;
  cues.askedAt = Date.now();
  const after = subtitleAfter(cues.lastEnd, s.position);
  let doc: string | null = null;
  try {
    const r = await fetch(
      `/api/vod/subtitles/${encodeURIComponent(s.viewer)}/${index}?after=${after.toFixed(3)}`,
      requestInit("GET"),
    );
    if (r.status === 401) signedOutNow();
    if (r.ok) doc = await r.text();
  } catch {
    // Asked again in a moment.
  }
  cues.asking = false;
  const start = s.start;
  const still = session === s && s.reading === reading && s.subtitle === index && s.cues === cues;
  if (doc === null || !still || start === null) return;
  const track = textTrack(el);
  track.mode = "showing";
  for (const c of parseVtt(doc)) {
    const key = `${c.start}|${c.end}|${c.text}`;
    cues.lastEnd = Math.max(cues.lastEnd ?? c.end, c.end);
    if (cues.held.has(key)) continue;
    const t = cueTimes(c, start);
    if (!t) continue;
    cues.held.add(key);
    try {
      track.addCue(new VTTCue(t.start, t.end, c.text));
    } catch {
      // A browser without VTTCue shows none.
    }
  }
}

// ------------------------------------------------------------- the stream

interface Sink {
  data(chunk: ArrayBuffer, at: number): void;
  complete(received: number): void;
  fail(status: number, message: string): void;
}

/** How much of a remux is kept after it has been handed on, for a loader
 *  that resumes from a little before where the last one stopped. */
const KEEP_BYTES = 16 * 1024 * 1024;

/** One remux response. mpegts.js stops reading when it holds enough ahead
 *  (`lazyLoad`) or its buffer is full, by dropping its loader, and resumes
 *  with a new one from a byte offset: here that is the same response read
 *  on, which waits meanwhile, and the remux with it. Its `X-Start` header
 *  says where in the film it really starts. */
class Reading {
  private abort = new AbortController();
  private reader: ReadableStreamDefaultReader<Uint8Array> | null = null;
  private begun = false;
  private kept: { at: number; bytes: ArrayBuffer }[] = [];
  private keptBytes = 0;
  private received = 0;
  private pumping = false;
  private done = false;
  private sink: Sink | null = null;
  /** What the service answered instead of a stream. */
  refused: { status: number; body: unknown } | null = null;

  constructor(
    readonly url: string,
    private onStart: (seconds: number) => void,
  ) {}

  /** Hands the response on to `sink`, from byte `from`. False when that is
   *  further back than what is kept. */
  attach(sink: Sink, from: number): boolean {
    this.sink = sink;
    if (!this.begun) {
      this.begun = true;
      void this.begin();
      return true;
    }
    const first = this.kept[0]?.at ?? this.received;
    if (from < first || from > this.received) return false;
    for (const k of this.kept) {
      const end = k.at + k.bytes.byteLength;
      if (end <= from) continue;
      const skip = Math.max(0, from - k.at);
      sink.data(skip > 0 ? k.bytes.slice(skip) : k.bytes, k.at + skip);
    }
    if (this.done) sink.complete(this.received);
    else void this.pump();
    return true;
  }

  detach(sink: Sink) {
    if (this.sink === sink) this.sink = null;
  }

  cancel() {
    this.sink = null;
    this.abort.abort();
  }

  private async begin() {
    let resp: Response;
    try {
      resp = await fetch(this.url, {
        credentials: "same-origin",
        cache: "no-store",
        signal: this.abort.signal,
      });
    } catch (e) {
      if (!this.abort.signal.aborted) this.sink?.fail(0, String(e));
      return;
    }
    if (!resp.ok || !resp.body) {
      this.refused = { status: resp.status, body: await resp.json().catch(() => null) };
      this.sink?.fail(resp.status, resp.statusText);
      return;
    }
    const at = Number(resp.headers.get("X-Start"));
    if (resp.headers.has("X-Start") && Number.isFinite(at)) this.onStart(at);
    this.reader = resp.body.getReader();
    void this.pump();
  }

  /** Reads on while somebody takes what is read. */
  private async pump() {
    if (this.pumping || !this.reader) return;
    this.pumping = true;
    try {
      while (this.sink && !this.done) {
        const r = await this.reader.read();
        if (r.done) {
          this.done = true;
          this.sink?.complete(this.received);
          break;
        }
        const v = r.value;
        const bytes =
          v.byteOffset === 0 && v.byteLength === v.buffer.byteLength
            ? (v.buffer as ArrayBuffer)
            : (v.slice().buffer as ArrayBuffer);
        const at = this.received;
        this.received += bytes.byteLength;
        this.kept.push({ at, bytes });
        this.keptBytes += bytes.byteLength;
        while (this.keptBytes > KEEP_BYTES && this.kept.length > 1) {
          this.keptBytes -= this.kept.shift()!.bytes.byteLength;
        }
        this.sink?.data(bytes, at);
      }
    } catch (e) {
      if (!this.abort.signal.aborted) this.sink?.fail(0, String(e));
    } finally {
      this.pumping = false;
    }
  }
}

/** mpegts.js's loader for `reading`: each one it makes reads the same
 *  response, from where it asks. */
function loaderFor(mpegts: Mpegts, reading: Reading) {
  const { BaseLoader, LoaderStatus, LoaderErrors } = mpegts;
  return class VodLoader extends BaseLoader {
    private sink: Sink | null = null;

    constructor() {
      super("vod-loader");
      this._needStash = true;
    }

    open(_source: unknown, range: { from: number; to: number }) {
      this._status = LoaderStatus.kConnecting;
      let got = 0;
      const sink: Sink = {
        data: (chunk, at) => {
          this._status = LoaderStatus.kBuffering;
          got += chunk.byteLength;
          this.onDataArrival?.(chunk, at, got);
        },
        complete: (received) => {
          this._status = LoaderStatus.kComplete;
          this.onComplete?.(range.from, received - 1);
        },
        fail: (status, message) => {
          this._status = LoaderStatus.kError;
          // mpegts.js types its error kinds oddly: they are its strings.
          const kind = (status > 0
            ? LoaderErrors.HTTP_STATUS_CODE_INVALID
            : LoaderErrors.EXCEPTION) as unknown as LoaderError;
          this.onError?.(kind, { code: status || -1, msg: message });
        },
      };
      this.sink = sink;
      if (!reading.attach(sink, range.from)) {
        sink.fail(0, "The remux can't be read from there again.");
      }
    }

    abort() {
      if (this.sink) reading.detach(this.sink);
      this.sink = null;
      if (this._status !== LoaderStatus.kError) this._status = LoaderStatus.kComplete;
    }

    destroy() {
      this.abort();
      super.destroy();
    }
  };
}
