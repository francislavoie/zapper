// Things more than one pane can do. Kept out of components so the list, the
// palette, the guide and the chrome all fail the same way.

import type { Dispatch } from "react";
import { backend, capabilities, signOut } from "./backend";
import { toggleFullscreen } from "./fullscreen";
import { errorMessage, failed, loading, ready } from "./logic/async";
import { anchorFor, catchupMinutes } from "./logic/guide";
import { focusTarget } from "./logic/railItems";
import { guideOffsetFor, hitAction, inGuideReach } from "./logic/programmes";
import type { ActionId } from "./logic/palette";
import type { SleepRequest } from "./logic/sleep";
import { refreshFullscreen } from "./hooks/useFullscreen";
import { clearMovieDetails } from "./hooks/useMovieDetails";
import { clearShowDetails } from "./hooks/useShowDetails";
import { clearDeadPosters } from "./deadPosters";
import { windowControls } from "./window";
import { ALL_CATEGORIES, run, type Action, type Mode } from "./store";
import type {
  ChannelDto,
  EpisodeDto,
  FavouriteDto,
  MovieDto,
  PlayerStateDto,
  PlayingEpisodeDto,
  ProgrammeDto,
  ProgrammeHitDto,
  RecordingDto,
  SeriesDto,
} from "./types";
import { episodeCode } from "./logic/shows";
import {
  conflictLosesProgramme,
  conflictText,
  recordingWhen,
  scheduleRequest,
  type PlayerRecord,
} from "./logic/recordings";
import { NO_PLAYBACK } from "./logic/web";
import { FAVOURITE_LIMIT, RECENT_LIMIT } from "./logic/favourites";

export interface PlayOptions {
  /** Take over the window as well as starting the stream. Off by default:
   *  starting a channel is not a decision to stop looking at the list. */
  full?: boolean;
  /** Where a failed open should leave the user, when `full` took them
   *  somewhere. Whichever view they clicked from, not an arbitrary one. */
  back?: Mode;
}

/** Starts a channel.
 *
 *  Does not change what you are looking at. Throwing the window into the
 *  player on every pick would make the docked picture and "browsing never
 *  stops the stream" pointless — the one thing you could not do would be to
 *  start something and carry on looking. Full screen is a separate intent: a
 *  double-click, the picture itself, or `full: true`. */
export async function playChannel(
  dispatch: Dispatch<Action>,
  ch: ChannelDto,
  opts: PlayOptions = {},
) {
  // Separators are inert by construction; nothing should be able to open one.
  if (ch.isSeparator) return;
  // Where nothing plays (a browser without Media Source Extensions), say so
  // rather than pretend to open it: the picture slot would only sit there
  // empty.
  if (!capabilities.playback) {
    dispatch({ type: "toast", text: NO_PLAYBACK, kind: "info" });
    return;
  }

  // Metadata paints immediately — name, logo, programme — while the stream
  // opens, which is the whole trick for perceived latency.
  dispatch({ type: "opening", channel: ch });
  if (opts.full) dispatch({ type: "mode", mode: "watch" });
  const ok = await run(dispatch, `play ${ch.cleanName}`, backend.play(ch.streamId));
  // A failed open should not strand the user in a player with nothing in it.
  // Only worth undoing if this call is what put them there.
  if (!ok && opts.full) dispatch({ type: "mode", mode: opts.back ?? "browse" });
}

/** Back to the channel before this one, like a remote's recall button.
 *
 *  Starting it makes the current channel the previous one (see "opening" in
 *  the store), so a second press comes back. Like any other pick, it leaves
 *  the view alone: from the player it changes what the player shows, from
 *  the list what the docked picture does. */
export function playPrevious(dispatch: Dispatch<Action>, previous: ChannelDto | null) {
  if (!previous) {
    dispatch({ type: "toast", text: "No previous channel yet", kind: "info" });
    return;
  }
  void playChannel(dispatch, previous);
}

/** Sets the sleep timer, or turns it off with `null`. The reducer closes
 *  whichever menu it was chosen from. */
export function setSleep(dispatch: Dispatch<Action>, request: SleepRequest | null) {
  dispatch({ type: "sleep", request, at: Date.now() });
}

/** Toggles fullscreen from wherever the picture is.
 *
 *  From the docked picture that means going through the player: the video
 *  rect is measured from whichever slot the layout offers, and browse offers
 *  the corner, so without the mode change the window would go fullscreen
 *  with the picture still in its corner. Remembering where it came from is
 *  what lets leaving fullscreen go back there. */
export function togglePictureFullscreen(dispatch: Dispatch<Action>, mode: Mode) {
  if (mode !== "watch") {
    dispatch({ type: "fullscreenFrom", mode });
    dispatch({ type: "mode", mode: "watch" });
  }
  void toggleFullscreen()
    .then(refreshFullscreen)
    .catch((err: unknown) => dispatch({ type: "toast", text: `fullscreen: ${String(err)}` }));
}

// ------------------------------------------------------------------ actions

/** What an action needs beyond the dispatcher: the player, because "toggle
 *  compressor" has to read the current value to flip it, and the previous
 *  channel, for recall. */
export interface ActionContext {
  dispatch: Dispatch<Action>;
  player: PlayerStateDto;
  previous: ChannelDto | null;
  /** What Record from the player would do (logic/recordings.ts
   *  `playerRecord`), where Record can be chosen. */
  record?: PlayerRecord | null;
}

/** One behaviour per action id. `Record<ActionId, …>` makes the compiler
 *  reject both halves of the drift: an action the palette offers that nothing
 *  performs, and a behaviour nothing can reach. The command palette and the
 *  toolbar's hamburger both come through here, so the same entry cannot mean
 *  two different things depending on where it was clicked. */
const HANDLERS: Record<ActionId, (ctx: ActionContext) => void> = {
  browse: ({ dispatch }) => dispatch({ type: "mode", mode: "browse" }),
  guide: ({ dispatch }) => dispatch({ type: "mode", mode: "guide" }),
  previous: ({ dispatch, previous }) => playPrevious(dispatch, previous),
  compressor: ({ dispatch, player }) =>
    void run(dispatch, "compressor", backend.setCompressor(!player.compressor)),
  live: ({ dispatch }) => void run(dispatch, "seek to live", backend.seekLive()),
  pause: ({ dispatch }) => void run(dispatch, "pause", backend.togglePause()),
  stop: ({ dispatch }) => {
    void run(dispatch, "stop", backend.stop());
    dispatch({ type: "mode", mode: "browse" });
  },
  fullscreen: ({ dispatch }) =>
    void toggleFullscreen().catch((e: unknown) =>
      dispatch({ type: "toast", text: `fullscreen: ${String(e)}` }),
    ),
  theater: ({ dispatch }) => dispatch({ type: "mode", mode: "watch" }),
  stats: ({ dispatch }) => dispatch({ type: "stats", open: true }),
  sleep15: ({ dispatch }) => setSleep(dispatch, 15),
  sleep30: ({ dispatch }) => setSleep(dispatch, 30),
  sleep45: ({ dispatch }) => setSleep(dispatch, 45),
  sleep60: ({ dispatch }) => setSleep(dispatch, 60),
  sleep90: ({ dispatch }) => setSleep(dispatch, 90),
  sleepProgramme: ({ dispatch }) => setSleep(dispatch, "programme"),
  sleepOff: ({ dispatch }) => setSleep(dispatch, null),
  sync: ({ dispatch }) => void run(dispatch, "sync", backend.syncNow()),
  record: ({ dispatch, record }) => void recordFromPlayer(dispatch, record ?? null),
  recordings: ({ dispatch }) => dispatch({ type: "showRecordings" }),
  movies: ({ dispatch }) => dispatch({ type: "section", section: "movies" }),
  shows: ({ dispatch }) => dispatch({ type: "section", section: "shows" }),
  liveTv: ({ dispatch }) => dispatch({ type: "section", section: "live" }),
  settings: ({ dispatch }) => dispatch({ type: "settings", open: true }),
  updates: ({ dispatch }) => {
    dispatch({ type: "updateDialog", open: true });
    void checkForUpdates(dispatch);
  },
  about: ({ dispatch }) => dispatch({ type: "about", open: true }),
  // One window, so closing it is quitting; the close path already saves the
  // session and the window size on the way out.
  quit: ({ dispatch }) =>
    void windowControls
      .close()
      .catch((e: unknown) => dispatch({ type: "toast", text: `quit: ${String(e)}` })),
  // The sign-in page follows by itself (WebGate): the service forgets this
  // browser, and its answer clears the cookie.
  signOut: ({ dispatch }) =>
    void signOut().catch((e: unknown) =>
      dispatch({ type: "toast", text: `sign out: ${errorMessage(e)}` }),
    ),
};

/** Asks GitHub now, whatever the automatic-check setting says: asking is
 *  consent. The answer arrives through the update subscription as well as
 *  here; a failure is a toast, because a failed check changes no state for
 *  the dialog to show. A build that is not configured answers at once
 *  without asking anything, which the dialog then says. */
export async function checkForUpdates(dispatch: Dispatch<Action>): Promise<void> {
  try {
    dispatch({ type: "update", value: await backend.updateCheck() });
  } catch (e) {
    dispatch({ type: "toast", text: `Check for updates: ${errorMessage(e)}` });
  }
}

export function runAction(id: ActionId, ctx: ActionContext): void {
  HANDLERS[id](ctx);
}

/** A programme that has already aired, on a channel with an archive. The
 *  backend takes the programme's own start and a length in whole minutes. */
export async function playCatchupProgramme(
  dispatch: Dispatch<Action>,
  ch: ChannelDto,
  programme: ProgrammeDto,
  back: Mode = "guide",
) {
  if (ch.isSeparator || !ch.hasArchive) return;

  dispatch({ type: "opening", channel: { ...ch, nowPlaying: programme } });
  const ok = await run(
    dispatch,
    `catchup ${ch.cleanName}`,
    backend.playCatchup(ch.streamId, programme.start, catchupMinutes(programme)),
  );
  if (!ok) dispatch({ type: "mode", mode: back });
}

/** What opening a programme hit needs to know about where the user is. */
export interface HitContext {
  /** The category on screen, so the guide keeps it when the channel is in it. */
  categoryId: string;
  /** Where a catch-up that fails to open should leave the user. */
  mode: Mode;
  guideHours: number;
  /** The stream ids of the favourites or recent list on screen, else null. */
  listed: readonly number[] | null;
}

/** Chooses a programme search result: tunes to it if it is on, plays it from
 *  the archive if it has finished, and otherwise opens the guide at its time
 *  with its channel's row scrolled to and marked. */
export async function openProgrammeHit(
  dispatch: Dispatch<Action>,
  hit: ProgrammeHitDto,
  ctx: HitContext,
): Promise<void> {
  const action = hitAction(hit.airing);
  if (action === "tune") {
    void playChannel(dispatch, { ...hit.channel, nowPlaying: hit.programme });
    return;
  }
  // Where the archive can't be played, the guide shows the programme.
  if (action === "catchup" && capabilities.catchup) {
    void playCatchupProgramme(dispatch, hit.channel, hit.programme, ctx.mode);
    return;
  }

  const now = Math.floor(Date.now() / 1000);
  const anchor = anchorFor(now);
  const target = focusTarget(ctx.categoryId, hit.channel, ctx.listed);
  const categoryId = target.categoryId;
  // Not finding the row still opens the guide at the right time, which is
  // most of the answer.
  const index = target.lookUp ? await rowIn(categoryId, hit.channel.streamId) : target.index;
  dispatch({
    type: "guideFocus",
    categoryId,
    streamId: hit.channel.streamId,
    index,
    offset: guideOffsetFor(hit.programme.start, anchor, ctx.guideHours),
    anchor,
  });
  if (!inGuideReach(hit.programme.start, now)) {
    dispatch({
      type: "toast",
      kind: "info",
      text: `The guide shows two days ahead; ${hit.programme.title} is later than that`,
    });
  }
}

/** Where a channel's row is in a category, or null when that cannot be
 *  found: the row is looked up rather than paged to, and not finding it
 *  still lands on the right list. */
async function rowIn(categoryId: string, streamId: number): Promise<number | null> {
  try {
    return await backend.channelIndex(categoryId === ALL_CATEGORIES ? null : categoryId, streamId);
  } catch {
    return null;
  }
}

/** Shows the channel's own category in the list, with its row in the middle
 *  and marked. */
export async function showChannelCategory(dispatch: Dispatch<Action>, channel: ChannelDto): Promise<void> {
  if (channel.categoryId === null) return;
  const index = await rowIn(channel.categoryId, channel.streamId);
  dispatch({ type: "focusChannel", categoryId: channel.categoryId, streamId: channel.streamId, index });
}

/** Opens the guide at now on the channel's row: in the favourites or recent
 *  list when it is in the one on screen, otherwise in its category. */
export async function showChannelInGuide(
  dispatch: Dispatch<Action>,
  channel: ChannelDto,
  ctx: { categoryId: string; listed: readonly number[] | null },
): Promise<void> {
  const anchor = anchorFor(Math.floor(Date.now() / 1000));
  const target = focusTarget(ctx.categoryId, channel, ctx.listed);
  const index = target.lookUp ? await rowIn(target.categoryId, channel.streamId) : target.index;
  dispatch({
    type: "guideFocus",
    categoryId: target.categoryId,
    streamId: channel.streamId,
    index,
    offset: 0,
    anchor,
  });
}

/** Fetches the category list into the store.
 *
 *  Here rather than inside CategoryRail because the catalogue is not that
 *  pane's private data: the favourites rail names each channel's
 *  category from it too. And a hidden `<Activity>` subtree does not run
 *  effects, so a load that lived in the rail simply never happened while the
 *  Favourites tab was the one on screen — which is exactly when the
 *  favourites needed it. */
/** `quiet` re-reads without first saying "Loading": a refresh after a sync
 *  keeps the rail as it is until the new list replaces it, rather than
 *  emptying it and losing its scroll. */
export async function loadCategories(dispatch: Dispatch<Action>, quiet = false) {
  if (!quiet) dispatch({ type: "categories", value: loading() });
  try {
    dispatch({ type: "categories", value: ready(await backend.categories()) });
  } catch (e) {
    dispatch({ type: "categories", value: failed(errorMessage(e)) });
  }
}

/** Fetches Favourites and Recently watched together. A list that is already
 *  showing stays as it is while the fresh one loads: the answer is nearly
 *  always the same list, slightly reordered, and a flash of "loading" would
 *  swap a good answer for a worse one. */
export async function loadWatchLists(dispatch: Dispatch<Action>): Promise<void> {
  const one = async (
    list: "favourites" | "recent",
    fetch: () => Promise<FavouriteDto[]>,
  ) => {
    dispatch({ type: "watchList", list, value: loading() });
    try {
      dispatch({ type: "watchList", list, value: ready(await fetch()) });
    } catch (e) {
      dispatch({ type: "watchList", list, value: failed(errorMessage(e)) });
    }
  };
  await Promise.all([
    one("favourites", () => backend.favourites(FAVOURITE_LIMIT)),
    one("recent", () => backend.recentlyWatched(RECENT_LIMIT)),
  ]);
}

/** Drops a channel from what was watched, then reloads both lists. */
export async function forgetChannel(dispatch: Dispatch<Action>, streamId: number): Promise<void> {
  dispatch({ type: "favouriteMenu", menu: null });
  const ok = await run(dispatch, "forget", backend.forgetActivity(streamId));
  if (ok) await loadWatchLists(dispatch);
}

// ----------------------------------------------------------------- movies

/** Opens a movie in the player, from `start` seconds or, with `null`, from
 *  the start. Like a channel it takes the window over (a movie is watched,
 *  not glanced at), and a failed open gives the window back. Choosing a movie
 *  to look at never comes through here: only Play does. */
export async function playMovie(
  dispatch: Dispatch<Action>,
  movie: MovieDto,
  start: number | null = null,
): Promise<void> {
  if (!capabilities.playback) {
    dispatch({ type: "toast", text: NO_PLAYBACK, kind: "info" });
    return;
  }
  dispatch({ type: "movies/playing", movie });
  dispatch({ type: "mode", mode: "watch" });
  const ok = await run(dispatch, `play ${movie.cleanName}`, backend.playMovie(movie.movieId, start));
  if (!ok) {
    dispatch({ type: "movies/playing", movie: null });
    dispatch({ type: "mode", mode: "browse" });
  }
}

/** Sets one mark and reports it, then re-reads the counts the rail shows. */
async function markMovie(
  dispatch: Dispatch<Action>,
  movie: MovieDto,
  what: string,
  marks: { favourite?: boolean; watched?: boolean },
): Promise<void> {
  try {
    dispatch({ type: "movies/marked", marks: await backend.setMovieMarks(movie.movieId, marks) });
  } catch (e) {
    dispatch({ type: "toast", text: `${what}: ${errorMessage(e)}` });
    return;
  }
  await loadMoviesSummary(dispatch, true);
}

export function toggleMovieFavourite(dispatch: Dispatch<Action>, movie: MovieDto): Promise<void> {
  return markMovie(dispatch, movie, "favourite", { favourite: !movie.favourite });
}

export function setMovieWatched(
  dispatch: Dispatch<Action>,
  movie: MovieDto,
  watched: boolean,
): Promise<void> {
  return markMovie(dispatch, movie, "watched", { watched });
}

/** Which catalogue the last summary's movies came from; `null` before one. */
let moviesOrigin: string | null = null;

/** The rail's counts, and whether the account has movies at all. `quiet`
 *  re-reads without first saying "Loading", as `loadCategories` does. When
 *  the movies are another account's, the details and the pictures that
 *  failed to load, kept by id and address, are forgotten (the store resets
 *  the rest). */
export async function loadMoviesSummary(dispatch: Dispatch<Action>, quiet = false): Promise<void> {
  if (!quiet) dispatch({ type: "movies/summary", value: loading() });
  try {
    const summary = await backend.moviesSummary();
    if (moviesOrigin !== null && moviesOrigin !== summary.origin) {
      clearMovieDetails();
      clearDeadPosters();
    }
    moviesOrigin = summary.origin;
    dispatch({ type: "movies/summary", value: ready(summary) });
  } catch (e) {
    dispatch({ type: "movies/summary", value: failed(errorMessage(e)) });
  }
}

export async function loadMovieCategories(
  dispatch: Dispatch<Action>,
  quiet = false,
): Promise<void> {
  if (!quiet) dispatch({ type: "movies/categories", value: loading() });
  try {
    dispatch({ type: "movies/categories", value: ready(await backend.movieCategories()) });
  } catch (e) {
    dispatch({ type: "movies/categories", value: failed(errorMessage(e)) });
  }
}

// ------------------------------------------------------------------ shows

/** Opens an episode in the player, from `start` seconds or, with `null`, from
 *  the start: as a movie does (see `playMovie`), and the show's sheet closes
 *  for the player. Only Play comes through here; choosing a show, opening
 *  its sheet or a season never does. */
export async function playEpisode(
  dispatch: Dispatch<Action>,
  playing: PlayingEpisodeDto,
  start: number | null = null,
): Promise<void> {
  if (!capabilities.playback) {
    dispatch({ type: "toast", text: NO_PLAYBACK, kind: "info" });
    return;
  }
  const e = playing.episode;
  dispatch({ type: "shows/playing", playing });
  dispatch({ type: "shows/sheet", id: null });
  dispatch({ type: "mode", mode: "watch" });
  const ok = await run(
    dispatch,
    `play ${playing.series.cleanName} ${episodeCode(e.season, e.episode, e.episodeEnd)}`,
    backend.playEpisode(e.episodeId, start),
  );
  if (!ok) {
    dispatch({ type: "shows/playing", playing: null });
    dispatch({ type: "mode", mode: "browse" });
  }
}

/** Sets one of a show's marks and reports it, then re-reads the counts the
 *  rail shows. Watched reaches every episode, so the show's details are read
 *  again too. */
async function markShow(
  dispatch: Dispatch<Action>,
  show: SeriesDto,
  what: string,
  marks: { favourite?: boolean; watched?: boolean },
): Promise<void> {
  try {
    dispatch({ type: "shows/marked", marks: await backend.setSeriesMarks(show.seriesId, marks) });
  } catch (e) {
    dispatch({ type: "toast", text: `${what}: ${errorMessage(e)}` });
    return;
  }
  if (marks.watched !== undefined) dispatch({ type: "shows/played", seriesId: show.seriesId });
  await loadShowsSummary(dispatch, true);
}

export function toggleShowFavourite(dispatch: Dispatch<Action>, show: SeriesDto): Promise<void> {
  return markShow(dispatch, show, "favourite", { favourite: !show.favourite });
}

export function setShowWatched(
  dispatch: Dispatch<Action>,
  show: SeriesDto,
  watched: boolean,
): Promise<void> {
  return markShow(dispatch, show, "watched", { watched });
}

/** Marks one episode watched or not. Its show's details (the season's
 *  count, what is next) and whether the show is watched follow from it, so
 *  both are read again. */
export async function setEpisodeWatched(
  dispatch: Dispatch<Action>,
  episode: EpisodeDto,
  watched: boolean,
): Promise<void> {
  try {
    const marks = await backend.setEpisodeMarks(episode.episodeId, watched);
    dispatch({ type: "shows/episodeMarked", seriesId: episode.seriesId, marks });
  } catch (e) {
    dispatch({ type: "toast", text: `watched: ${errorMessage(e)}` });
    return;
  }
  dispatch({ type: "shows/played", seriesId: episode.seriesId });
  await loadShowsSummary(dispatch, true);
}

/** Which catalogue the last summary's shows came from; `null` before one. */
let showsOrigin: string | null = null;

/** The rail's counts, and whether the account has shows at all, as
 *  `loadMoviesSummary`. When the shows are another account's, the details
 *  kept by id and the pictures that failed to load are forgotten (the store
 *  resets the rest). */
export async function loadShowsSummary(dispatch: Dispatch<Action>, quiet = false): Promise<void> {
  if (!quiet) dispatch({ type: "shows/summary", value: loading() });
  try {
    const summary = await backend.showsSummary();
    if (showsOrigin !== null && showsOrigin !== summary.origin) {
      clearShowDetails();
      clearDeadPosters();
    }
    showsOrigin = summary.origin;
    dispatch({ type: "shows/summary", value: ready(summary) });
  } catch (e) {
    dispatch({ type: "shows/summary", value: failed(errorMessage(e)) });
  }
}

export async function loadShowCategories(
  dispatch: Dispatch<Action>,
  quiet = false,
): Promise<void> {
  if (!quiet) dispatch({ type: "shows/categories", value: loading() });
  try {
    dispatch({ type: "shows/categories", value: ready(await backend.seriesCategories()) });
  } catch (e) {
    dispatch({ type: "shows/categories", value: failed(errorMessage(e)) });
  }
}

// ------------------------------------------------------------- recording

/** Asks the recording service for its recordings. A service that does not
 *  answer is not an error to shout about: it is simply not running, which
 *  the guide shows by having no red dots and the menu by saying so. */
export async function refreshRecordings(dispatch: Dispatch<Action>): Promise<void> {
  await fetchRecordings(dispatch);
}

/** Says something was just asked of the recording service, so the list is
 *  followed closely for a few seconds (RecordingsWatcher), and fetches it. */
async function afterRecordingAction(dispatch: Dispatch<Action>): Promise<void> {
  dispatch({ type: "recordingsTouched", at: Date.now() });
  await fetchRecordings(dispatch);
}

async function fetchRecordings(dispatch: Dispatch<Action>): Promise<void> {
  try {
    dispatch({ type: "recordings", list: await backend.recordings() });
  } catch (e) {
    dispatch({ type: "recordings", list: null, error: errorMessage(e) });
  }
}

/** Schedules a programme with the recording service. */
export async function recordProgramme(
  dispatch: Dispatch<Action>,
  ch: ChannelDto,
  p: ProgrammeDto,
): Promise<void> {
  dispatch({ type: "programmeMenu", menu: null });
  try {
    const rec = await backend.recordProgramme(scheduleRequest(ch, p));
    const now = Math.floor(Date.now() / 1000);
    const conflict = rec.conflicts?.[0];
    dispatch({
      type: "toast",
      kind: conflict && conflictLosesProgramme(conflict) ? "error" : "info",
      text: `Recording “${rec.title}”, ${recordingWhen(rec, now)}${conflict ? `. ${conflictText(conflict)}` : ""}`,
    });
  } catch (e) {
    dispatch({ type: "toast", text: `Record: ${errorMessage(e)}` });
  }
  await afterRecordingAction(dispatch);
}

/** Record from the player: the programme on now on the channel it shows.
 *  On the desktop, the app keeps what the player has already received of
 *  it, and on an account with no connection to spare moves the player onto
 *  the recording service's connection, which the recording shares
 *  (docs/recording.md, "Recording what you watch"). Already being recorded,
 *  it says so rather than stopping it: a stray key must not end a
 *  recording, which the menus' Stop recording does. */
export async function recordFromPlayer(
  dispatch: Dispatch<Action>,
  r: PlayerRecord | null,
): Promise<void> {
  if (!r) {
    dispatch({
      type: "toast",
      kind: "info",
      text: "Nothing to record: the guide says nothing about what is on, or the recording service is not running",
    });
    return;
  }
  if (r.kind === "record") {
    await recordProgramme(dispatch, r.channel, r.programme);
    return;
  }
  const what = r.kind === "recording" ? "Recording" : r.kind === "saving" ? "Saving" : "Will record";
  dispatch({ type: "toast", kind: "info", text: `${what} “${r.rec.title}”` });
}

/** Cancels a scheduled recording, or stops one being recorded (which keeps
 *  what it has). */
export async function cancelRecording(dispatch: Dispatch<Action>, rec: RecordingDto): Promise<void> {
  dispatch({ type: "programmeMenu", menu: null });
  try {
    await backend.cancelRecording(rec.id);
  } catch (e) {
    dispatch({ type: "toast", text: `Cancel recording: ${errorMessage(e)}` });
  }
  await afterRecordingAction(dispatch);
}

/** Chooses `streamId` (`null`: none) as the channel a recording falls back
 *  to when its own feed fails. While it records, the service uses it from
 *  the next failure on. */
export async function setRecordingBackup(
  dispatch: Dispatch<Action>,
  rec: RecordingDto,
  streamId: number | null,
): Promise<RecordingDto | null> {
  dispatch({ type: "programmeMenu", menu: null });
  let out: RecordingDto | null = null;
  try {
    out = await backend.setRecordingBackup(rec.id, streamId);
    dispatch({
      type: "toast",
      kind: "info",
      text:
        out.backupStreamId == null
          ? `“${rec.title}” has no backup channel now`
          : `“${rec.title}” falls back to ${out.backupName ?? `channel ${out.backupStreamId}`}`,
    });
  } catch (e) {
    dispatch({ type: "toast", text: `Backup channel: ${errorMessage(e)}` });
  }
  await afterRecordingAction(dispatch);
  return out;
}

/** Forgets a finished recording and deletes its files. */
export async function deleteRecording(dispatch: Dispatch<Action>, rec: RecordingDto): Promise<void> {
  try {
    await backend.deleteRecording(rec.id, true);
  } catch (e) {
    dispatch({ type: "toast", text: `Delete recording: ${errorMessage(e)}` });
  }
  await afterRecordingAction(dispatch);
}
