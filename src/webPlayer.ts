// The player in a browser signed in to the recording service: the calls the
// app makes of mpv (src/api.ts's play, stop, pause, seek, volume, tracks…),
// made of a <video> element instead.
//
// The service streams a channel as MPEG-TS from its own address
// (`/api/stream/{viewer}`, recording::watch), having decided what this
// browser needs done to it (audio it can't play becomes AAC; video it can't
// play is refused with the reason). mpegts.js turns that into what Media
// Source Extensions take. Its MIT/Apache code is bundled; nothing comes from
// another site, and the provider's address never reaches the page.
//
// Rewinding is what the browser holds: the player keeps the last
// `SCROLLBACK_SECONDS` behind the picture, and back, forward, pause and Live
// move within that. Pausing for long lets the stream go (and the account's
// connection with it); resuming starts again at live. The provider's
// archive (catch-up) is not offered here.
//
// Volume and mute are the page's own; the compressor is the browser's own
// (a Web Audio compressor between the element and the speakers). Nothing
// here touches the system's volume.

import type MpegtsType from "mpegts.js";
import { requestInit } from "./logic/web";
import {
  PAUSE_LIMIT_SECONDS,
  START_AHEAD_SECONDS,
  SCROLLBACK_SECONDS,
  UNSUPPORTED,
  aspectFit,
  atLiveEdge,
  browserCan,
  bufferPosition,
  canPlayHere,
  endingOf,
  liveTarget,
  meansAnyway,
  refusalState,
  seekTarget,
  type MseHost,
  type WatchNotice,
  type WatchRefusal,
} from "./logic/webPlayback";
import type {
  AspectMode,
  PlayerOptionsDto,
  PlayerStateDto,
  StreamEnding,
  TrackDto,
} from "./types";

type Mpegts = typeof MpegtsType;
type Player = ReturnType<Mpegts["createPlayer"]>;

// ------------------------------------------------------------------ feeds

type Listener<T> = (v: T) => void;

/** A value pushed to whoever listens, and kept for whoever asks. */
class Feed<T> {
  private listeners = new Set<Listener<T>>();
  constructor(public last: T) {}
  set(v: T) {
    this.last = v;
    for (const l of this.listeners) l(v);
  }
  on(f: Listener<T>): () => void {
    this.listeners.add(f);
    return () => this.listeners.delete(f);
  }
}

const STOPPED: PlayerStateDto = {
  state: "stopped",
  streamId: null,
  width: null,
  height: null,
  fps: null,
  codec: null,
  bufferedBack: 0,
  behindLive: 0,
  atLiveEdge: true,
  volume: 100,
  muted: false,
  compressor: false,
  error: null,
  liveJump: null,
  joiningLive: false,
  ending: null,
  reconnect: null,
  onDemand: null,
  finished: null,
};

const NO_OPTIONS: PlayerOptionsDto = {
  audio: [],
  subtitles: [],
  subtitlesVisible: false,
  aspect: "auto",
  deinterlace: "auto",
};

export const playerFeed = new Feed<PlayerStateDto>(STOPPED);
export const optionsFeed = new Feed<PlayerOptionsDto>(NO_OPTIONS);

/** What the page says beside the picture: a recording that needs this
 *  connection, or the sound left out. */
export interface WebNotice {
  recording: WatchNotice | null;
  silent: string | null;
}
export const noticeFeed = new Feed<WebNotice>({ recording: null, silent: null });

/** Changes what the player reports, telling listeners only when something
 *  did change. Shared with a movie (src/webVod.ts). */
export function patch(p: Partial<PlayerStateDto>) {
  const next = { ...playerFeed.last, ...p };
  const before = playerFeed.last;
  const same = (Object.keys(p) as (keyof PlayerStateDto)[]).every((k) => before[k] === next[k]);
  if (!same) playerFeed.set(next);
  if (video) video.dataset.state = next.state;
}

// ----------------------------------------------------------------- setup

const host = globalThis as unknown as MseHost;

/** Whether live TV plays in this browser at all. */
export const playableHere: boolean = canPlayHere(host);

let mpegtsLib: Promise<Mpegts> | null = null;
/** mpegts.js, fetched the first time something plays: a page that only
 *  browses never loads it. */
export const loadMpegts = (): Promise<Mpegts> =>
  (mpegtsLib ??= import("mpegts.js").then((m) => {
    const lib = (m.default ?? m) as Mpegts;
    lib.LoggingControl.enableAll = false;
    return lib;
  }));

let signedOut: () => void = () => {};
/** What to do when the service says this browser is not signed in. */
export function onPlayerSignedOut(f: () => void) {
  signedOut = f;
}

/** The service said this browser is not signed in (to a movie). */
export const signedOutNow = () => signedOut();

// ----------------------------------------------------------------- state

interface Track {
  index: number;
  codec: string;
  lang: string | null;
  described: boolean;
  impaired: boolean;
}

interface WatchAnswer {
  viewer: string;
  stream: string;
  video: string | null;
  audio: Track[];
  audioIndex: number | null;
  converting: boolean;
  silent: string | null;
  shared: boolean;
  notice: WatchNotice | null;
}

interface Session {
  seq: number;
  streamId: number;
  viewer: string;
  stream: string;
  tracks: Track[];
  audioIndex: number | null;
  silent: boolean;
  player: Player | null;
  /** Let go after a long pause; resuming starts again. */
  suspended: boolean;
  /** Kept at live: nobody has paused or rewound. */
  following: boolean;
  wasLive: boolean;
  /** Reopened after the browser's side of the stream broke, within the
   *  last minute. */
  reopens: number[];
  /** The picture has been started on this reading (`startWhenReady`). */
  started: boolean;
  openedAt: number;
}

let video: HTMLVideoElement | null = null;
let session: Session | null = null;
let seq = 0;
let jumps = 0;
/** The last channel that failed, and how: playing it again from the
 *  picture's "Watch here anyway" means exactly that. */
let lastFailure: { streamId: number; ending: StreamEnding | null } | null = null;
let aspect: AspectMode = "auto";
let ticker: ReturnType<typeof setInterval> | undefined;
let poller: ReturnType<typeof setInterval> | undefined;

// ----------------------------------------------------------- the element

/** The element the picture is drawn in, when the page has one. */
export const currentVideo = (): HTMLVideoElement | null => video;

/** The aspect chosen, which outlasts what is playing. */
export const currentAspect = (): AspectMode => aspect;

/** The <video> the picture is drawn in (App.tsx), placed where mpv's picture
 *  would be. */
export function attachVideo(el: HTMLVideoElement | null) {
  if (video === el) return;
  video = el;
  if (!el) return;
  el.playsInline = true;
  el.disablePictureInPicture = true;
  el.controls = false;
  el.dataset.state = playerFeed.last.state;
  el.muted = playerFeed.last.muted;
  applyVolume();
  el.addEventListener("playing", () => {
    if (session) patch({ state: "playing", joiningLive: false });
  });
  el.addEventListener("pause", () => {
    if (session && playerFeed.last.state === "playing") patch({ state: "paused" });
  });
  el.addEventListener("resize", () => {
    patch({ width: el.videoWidth || null, height: el.videoHeight || null });
    applyAspect();
  });
}

function ranges(): [number, number][] {
  const out: [number, number][] = [];
  const b = video?.buffered;
  if (!b) return out;
  for (let i = 0; i < b.length; i++) out.push([b.start(i), b.end(i)]);
  return out;
}

// ------------------------------------------------------------- playing

async function watchRequest(
  streamId: number,
  anyway: boolean,
  replaces: string | null,
  audio: number | null,
): Promise<{ status: number; body: unknown }> {
  try {
    const resp = await fetch(
      "/api/watch",
      requestInit("POST", { streamId, anyway, replaces, audio, can: browserCan(host) }),
    );
    const body: unknown = await resp.json().catch(() => null);
    return { status: resp.status, body };
  } catch {
    return { status: 0, body: null };
  }
}

/** Starts channel `streamId`. Resolves once the service has answered; what
 *  happens then (playing, or why not) comes through `playerFeed`, as the
 *  app's player reports through its events. `replacing`: a movie's viewer
 *  this one takes over from (src/webVod.ts), when no channel was on. */
export async function play(streamId: number, replacing: string | null = null): Promise<void> {
  if (!playableHere) throw new Error(UNSUPPORTED);
  const my = ++seq;
  const old = session;
  const anyway =
    lastFailure !== null && lastFailure.streamId === streamId && meansAnyway(lastFailure.ending);
  lastFailure = null;
  release(old, false);
  session = null;
  noticeFeed.set({ recording: null, silent: null });
  optionsFeed.set({ ...NO_OPTIONS, aspect });
  patch({
    state: "opening",
    streamId,
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
    onDemand: null,
    finished: null,
  });
  const lib = loadMpegts();
  const { status, body } = await watchRequest(streamId, anyway, old?.viewer ?? replacing, null);
  if (my !== seq) {
    // Superseded while asking: let the answer's viewer go at once.
    const late = body as Partial<WatchAnswer> | null;
    if (status === 200 && late?.viewer) void forget(late.viewer);
    return;
  }
  if (status !== 200) {
    if (status === 401) signedOut();
    const r = refusalState(status, body as WatchRefusal | null);
    fail(streamId, r.ending, r.error);
    return;
  }
  const a = body as WatchAnswer;
  const s: Session = {
    seq: my,
    streamId,
    viewer: a.viewer,
    stream: a.stream,
    tracks: a.audio ?? [],
    audioIndex: a.audioIndex ?? null,
    silent: a.silent != null,
    player: null,
    suspended: false,
    following: true,
    wasLive: true,
    reopens: [],
    started: false,
    openedAt: Date.now(),
  };
  session = s;
  optionsFeed.set(optionsOf(s));
  noticeFeed.set({ recording: a.notice ?? null, silent: a.silent ?? null });
  let mpegts: Mpegts;
  try {
    mpegts = await lib;
  } catch {
    if (my === seq) fail(streamId, null, "The player couldn't be loaded. Reload the page.");
    return;
  }
  if (my !== seq) return;
  open(s, mpegts, a.stream);
  startTimers();
}

/** Plays the session's stream in the element, from live. */
function open(s: Session, mpegts: Mpegts, url: string) {
  const el = video;
  if (!el) return;
  s.player?.destroy();
  const p = mpegts.createPlayer(
    {
      type: "mpegts",
      isLive: true,
      url: new URL(url, location.href).href,
      ...(s.silent || (s.tracks.length === 0 && s.audioIndex === null) ? { hasAudio: false } : {}),
    },
    {
      enableWorker: false,
      enableStashBuffer: false,
      lazyLoad: false,
      liveBufferLatencyChasing: false,
      autoCleanupSourceBuffer: true,
      autoCleanupMaxBackwardDuration: SCROLLBACK_SECONDS,
      autoCleanupMinBackwardDuration: SCROLLBACK_SECONDS - 30,
    },
  );
  p.on(mpegts.Events.ERROR, () => void broke(s));
  p.on(mpegts.Events.LOADING_COMPLETE, () => void broke(s));
  p.on(mpegts.Events.MEDIA_INFO, (mi: { videoCodec?: string; audioCodec?: string; fps?: number }) => {
    if (session !== s) return;
    patch({ codec: mi.videoCodec ?? null, fps: mi.fps ?? null });
    el.dataset.audioCodec = mi.audioCodec ?? "";
    el.dataset.videoCodec = mi.videoCodec ?? "";
  });
  s.player = p;
  s.started = false;
  s.openedAt = Date.now();
  p.attachMediaElement(el);
  p.load();
  // The first frame waits for a few seconds in hand (`tick` starts it), as
  // the app's "Joining live…" does: started on the very newest frame, every
  // late packet would stall it.
  patch({ joiningLive: true });
}

/** Starts the picture once there is enough ahead of it. */
function startWhenReady(s: Session, el: HTMLVideoElement, ahead: number) {
  const waited = Date.now() - s.openedAt;
  if (ahead < START_AHEAD_SECONDS && !(waited > 8000 && ahead > 0.5)) return;
  s.started = true;
  patch({ joiningLive: false });
  el.play()?.catch(() => {
    // Refused without a click behind it (a page reloaded into playing): the
    // picture waits for Play.
    if (session === s) patch({ state: "paused" });
  });
}

/** The browser's side of the stream ended or failed: ask the service why.
 *  When it says the feed is fine, the reading is opened again (a few times
 *  a minute at most); otherwise the picture says what happened. */
async function broke(s: Session) {
  if (session !== s || s.suspended) return;
  const st = await viewerStatus(s);
  if (session !== s) return;
  if (st?.ended) {
    if (st.takenOver) {
      fail(s.streamId, { kind: "inUse", slots: null, status: null, limit: false, clipSecs: null }, "Another browser started watching on this account's connection.");
    } else {
      fail(s.streamId, endingOf(st.ended), "The stream stopped.");
    }
    return;
  }
  const now = Date.now();
  s.reopens = s.reopens.filter((t) => now - t < 60_000);
  if (st === null || s.reopens.length >= 3) {
    fail(s.streamId, { kind: "network", slots: null, status: null, limit: false, clipSecs: null }, "The stream from the recording service broke off.");
    return;
  }
  s.reopens.push(now);
  const mpegts = await loadMpegts();
  if (session === s) open(s, mpegts, streamUrl(s));
}

function streamUrl(s: Session): string {
  return s.audioIndex !== null && s.audioIndex > 0 ? `${s.stream}?audio=${s.audioIndex}` : s.stream;
}

/** The picture stopped for a reason, shown over it. */
function fail(streamId: number, ending: StreamEnding | null, error: string) {
  const s = session;
  session = null;
  release(s, true);
  lastFailure = { streamId, ending };
  stopTimers();
  noticeFeed.set({ recording: null, silent: null });
  patch({ state: "failed", streamId, ending, error, reconnect: null, joiningLive: false });
}

/** Lets a session's stream go: its player, and with `forgetViewer` its
 *  place on the service (otherwise the next request replaces it). */
function release(s: Session | null, forgetViewer: boolean) {
  if (!s) return;
  try {
    s.player?.destroy();
  } catch {
    // Already torn down.
  }
  s.player = null;
  if (forgetViewer) void forget(s.viewer);
}

/** Lets the service's viewer go. `keepalive`: sent even as the page goes. */
async function forget(viewer: string, keepalive = false) {
  try {
    await fetch(`/api/watch/${encodeURIComponent(viewer)}`, {
      ...requestInit("DELETE"),
      keepalive,
    });
  } catch {
    // The service lets an unread viewer go by itself.
  }
}

interface ViewerStatus {
  ended: { kind: string; status: number | null; limit: boolean; clipSecs?: number | null } | null;
  takenOver: boolean;
  notice: WatchNotice | null;
}

async function viewerStatus(s: Session): Promise<ViewerStatus | null> {
  try {
    const r = await fetch(`/api/watch/${encodeURIComponent(s.viewer)}`, requestInit("GET"));
    if (r.status === 401) signedOut();
    if (r.status === 404 || r.status === 410) {
      return { ended: { kind: "dropped", status: null, limit: false }, takenOver: false, notice: null };
    }
    if (!r.ok) return null;
    return (await r.json()) as ViewerStatus;
  } catch {
    return null;
  }
}

/** Gives the channel up for a movie: its stream goes at once, and its
 *  viewer is returned for the movie's watch to replace (and so let go).
 *  `null` when no channel was on. */
export function takeLive(): string | null {
  seq++;
  const s = session;
  session = null;
  lastFailure = null;
  release(s, false);
  stopTimers();
  return s && !s.suspended ? s.viewer : null;
}

export async function stop(): Promise<void> {
  seq++;
  const s = session;
  session = null;
  lastFailure = null;
  release(s, true);
  stopTimers();
  noticeFeed.set({ recording: null, silent: null });
  optionsFeed.set({ ...NO_OPTIONS, aspect });
  if (video) {
    video.removeAttribute("src");
    video.load();
  }
  const { volume, muted, compressor } = playerFeed.last;
  playerFeed.set({ ...STOPPED, volume, muted, compressor });
}

// ------------------------------------------------------------- timers

function startTimers() {
  stopTimers();
  ticker = setInterval(tick, 500);
  poller = setInterval(() => void poll(), 5000);
}

function stopTimers() {
  if (ticker !== undefined) clearInterval(ticker);
  if (poller !== undefined) clearInterval(poller);
  ticker = poller = undefined;
}

/** Where the picture is in the buffer; keeping it at live while nobody has
 *  paused or rewound; letting a long pause go. */
function tick() {
  const s = session;
  const el = video;
  if (!s || !el || s.suspended) return;
  const r = ranges();
  if (!s.started) {
    const end = r.length > 0 ? r[r.length - 1][1] : 0;
    startWhenReady(s, el, end - el.currentTime);
    return;
  }
  const pos = bufferPosition(r, el.currentTime);
  if (s.following && !el.paused && pos.behindLive > 6) {
    // Stalls put it behind: back to live, and say so, as the app does.
    const to = liveTarget(r);
    if (to !== null) {
      el.currentTime = to;
      jumps += 1;
      patch({ liveJump: { seconds: Math.round(pos.behindLive * 10) / 10, seq: jumps } });
      return;
    }
  }
  if (el.paused && pos.behindLive > PAUSE_LIMIT_SECONDS) {
    suspend(s);
    return;
  }
  s.wasLive = atLiveEdge(s.wasLive, pos.behindLive);
  patch({
    bufferedBack: Math.round(pos.bufferedBack * 10) / 10,
    behindLive: Math.round(pos.behindLive * 10) / 10,
    atLiveEdge: s.wasLive,
  });
}

/** Paused too long, or the page going: the stream goes, the frame stays.
 *  `leaving`: the page is being closed or put away, and letting the
 *  viewer go must outlive it. */
function suspend(s: Session, leaving = false) {
  s.suspended = true;
  try {
    s.player?.destroy();
  } catch {
    // Gone already.
  }
  s.player = null;
  void forget(s.viewer, leaving);
  patch({ state: "paused", bufferedBack: 0, behindLive: 0, atLiveEdge: true });
}

// A closed tab would otherwise hold its channel's connection until the
// service notices nobody reads it. Let go as the page goes; a page restored
// from the browser's back-forward cache starts the channel again when
// played, as after a long pause.
if (typeof window !== "undefined") {
  window.addEventListener("pagehide", () => {
    const s = session;
    if (s && !s.suspended) suspend(s, true);
  });
}

/** What the service says about this viewer: a recording it is in the way
 *  of, or that it ended. A page nobody looks at gives the connection up as
 *  soon as a recording waits for it. */
async function poll() {
  const s = session;
  if (!s || s.suspended) return;
  const st = await viewerStatus(s);
  if (session !== s || !st) return;
  if (st.ended) {
    void broke(s);
    return;
  }
  noticeFeed.set({ recording: st.notice, silent: noticeFeed.last.silent });
  if (st.notice?.waiting && typeof document !== "undefined" && document.hidden) {
    await stop();
  }
}

// ------------------------------------------------------------- controls

export async function togglePause(): Promise<void> {
  const s = session;
  const el = video;
  if (!s || !el) return;
  if (s.suspended) {
    await play(s.streamId);
    return;
  }
  if (el.paused) {
    await el.play().catch(() => {});
  } else {
    s.following = false;
    el.pause();
  }
}

/** Pauses the channel or plays it, as asked; nothing when it already is. */
export async function setPaused(paused: boolean): Promise<void> {
  const s = session;
  const el = video;
  if (!s || !el) return;
  if ((s.suspended || el.paused) !== paused) await togglePause();
}

export async function seekRelative(seconds: number): Promise<void> {
  const s = session;
  const el = video;
  if (!s || !el || s.suspended) return;
  const to = seekTarget(ranges(), el.currentTime, seconds);
  if (to === null) return;
  el.currentTime = to;
  const pos = bufferPosition(ranges(), to);
  s.following = pos.behindLive < 3 && !el.paused;
  tick();
}

export async function seekLive(): Promise<void> {
  const s = session;
  const el = video;
  if (!s || !el) return;
  if (s.suspended) {
    await play(s.streamId);
    return;
  }
  const r = ranges();
  const to = liveTarget(r);
  if (to === null) return;
  const by = bufferPosition(r, el.currentTime).behindLive;
  el.currentTime = to;
  s.following = true;
  if (el.paused) await el.play().catch(() => {});
  if (by >= 1) {
    jumps += 1;
    patch({ liveJump: { seconds: Math.round(by * 10) / 10, seq: jumps } });
  }
  tick();
}

// ---------------------------------------------------------------- audio

interface Graph {
  ctx: AudioContext;
  source: MediaElementAudioSourceNode;
  compressor: DynamicsCompressorNode;
  gain: GainNode;
}
let graph: Graph | null = null;

/** The element's sound through Web Audio, made the first time the
 *  compressor is asked for (a click, which lets the context start). */
function audioGraph(): Graph | null {
  if (graph || !video) return graph;
  const Ctx =
    (globalThis as { AudioContext?: typeof AudioContext }).AudioContext ??
    (globalThis as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
  if (!Ctx) return null;
  const ctx = new Ctx();
  const source = ctx.createMediaElementSource(video);
  const compressor = ctx.createDynamicsCompressor();
  // Night listening: quiet passages up, loud ones down, as the app's
  // compressor sets out to do.
  compressor.threshold.value = -30;
  compressor.knee.value = 20;
  compressor.ratio.value = 4;
  compressor.attack.value = 0.01;
  compressor.release.value = 0.25;
  const gain = ctx.createGain();
  graph = { ctx, source, compressor, gain };
  route();
  return graph;
}

function route() {
  if (!graph) return;
  const { ctx, source, compressor, gain } = graph;
  source.disconnect();
  compressor.disconnect();
  if (playerFeed.last.compressor) {
    source.connect(compressor);
    compressor.connect(gain);
  } else {
    source.connect(gain);
  }
  gain.connect(ctx.destination);
  void ctx.resume().catch(() => {});
}

/** The page's own volume: the element's, or the graph's gain once there is
 *  one (an element's volume can't go above 1, nor on iOS be set at all). */
function applyVolume() {
  const { volume, muted } = playerFeed.last;
  const level = Math.max(0, volume) / 100;
  if (video) {
    video.muted = muted;
    if (!graph) video.volume = Math.min(1, level);
    else video.volume = 1;
  }
  if (graph) graph.gain.gain.value = muted ? 0 : level;
}

export async function setVolume(volume: number): Promise<void> {
  patch({ volume });
  applyVolume();
}

export async function setMuted(muted: boolean): Promise<void> {
  patch({ muted });
  applyVolume();
}

export async function setCompressor(enabled: boolean): Promise<void> {
  patch({ compressor: enabled });
  if (enabled) audioGraph();
  route();
  applyVolume();
}

// ---------------------------------------------------------------- tracks

function optionsOf(s: Session): PlayerOptionsDto {
  const audio: TrackDto[] = s.tracks.map((t) => ({
    id: t.index + 1,
    title: null,
    lang: t.lang,
    codec: t.codec,
    channels: null,
    default: t.index === 0,
    forced: false,
    hearingImpaired: t.impaired,
    visualImpaired: t.described,
    selected: s.audioIndex === t.index,
  }));
  return { audio, subtitles: [], subtitlesVisible: false, aspect, deinterlace: "auto" };
}

/** Another audio track: the service sends that one (converted if need be),
 *  from live. */
export async function setAudioTrack(id: number): Promise<PlayerOptionsDto> {
  const s = session;
  if (!s) return optionsFeed.last;
  const index = id - 1;
  if (index === s.audioIndex || !s.tracks.some((t) => t.index === index)) return optionsFeed.last;
  s.audioIndex = index;
  s.silent = false;
  const mpegts = await loadMpegts();
  if (session !== s) return optionsFeed.last;
  s.following = true;
  patch({ state: "opening" });
  open(s, mpegts, streamUrl(s));
  const o = optionsOf(s);
  optionsFeed.set(o);
  return o;
}

export async function setAspect(mode: AspectMode): Promise<PlayerOptionsDto> {
  aspect = mode;
  applyAspect();
  const o = { ...optionsFeed.last, aspect };
  optionsFeed.set(o);
  return o;
}

/** Fits the picture to its box for the aspect chosen. Called as the box
 *  moves too (App.tsx), since a forced shape depends on the box's. */
export function applyAspect() {
  const el = video;
  if (!el) return;
  const box = el.getBoundingClientRect();
  const native = el.videoWidth > 0 && el.videoHeight > 0 ? el.videoWidth / el.videoHeight : 0;
  const f = aspectFit(aspect, { width: box.width, height: box.height }, native);
  el.style.objectFit = f.fit;
  el.style.transform = f.sx === 1 && f.sy === 1 ? "" : `scale(${f.sx}, ${f.sy})`;
}

export const options = (): PlayerOptionsDto => optionsFeed.last;
export const state = (): PlayerStateDto => playerFeed.last;
