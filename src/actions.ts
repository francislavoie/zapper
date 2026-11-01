// Things more than one pane can do. Kept out of components so the list, the
// palette, the guide and the chrome all fail the same way.

import type { Dispatch } from "react";
import { backend } from "./backend";
import { toggleFullscreen } from "./fullscreen";
import { errorMessage, failed, loading, ready } from "./logic/async";
import { catchupMinutes } from "./logic/guide";
import type { ActionId } from "./logic/palette";
import type { SleepRequest } from "./logic/sleep";
import { refreshFullscreen } from "./hooks/useFullscreen";
import { run, type Action, type Mode } from "./store";
import type { ChannelDto, PlayerStateDto, ProgrammeDto } from "./types";

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
  settings: ({ dispatch }) => dispatch({ type: "settings", open: true }),
  updates: ({ dispatch }) => {
    dispatch({ type: "updateDialog", open: true });
    void checkForUpdates(dispatch);
  },
  about: ({ dispatch }) => dispatch({ type: "about", open: true }),
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

/** Fetches the category list into the store.
 *
 *  Here rather than inside CategoryRail because the catalogue is not that
 *  pane's private data: the favourites rail names each channel's
 *  category from it too. And a hidden `<Activity>` subtree does not run
 *  effects, so a load that lived in the rail simply never happened while the
 *  Favourites tab was the one on screen — which is exactly when the
 *  favourites needed it. */
export async function loadCategories(dispatch: Dispatch<Action>) {
  dispatch({ type: "categories", value: loading() });
  try {
    dispatch({ type: "categories", value: ready(await backend.categories()) });
  } catch (e) {
    dispatch({ type: "categories", value: failed(errorMessage(e)) });
  }
}
