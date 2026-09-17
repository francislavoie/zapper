// Things more than one pane can do. Kept out of components so the list, the
// palette, the guide and the chrome all fail the same way.

import type { Dispatch } from "react";
import { backend } from "./backend";
import { toggleFullscreen } from "./fullscreen";
import { catchupMinutes } from "./logic/guide";
import type { ActionId } from "./logic/palette";
import { run, type Action, type Mode } from "./store";
import type { ChannelDto, PlayerStateDto, ProgrammeDto } from "./types";

export async function playChannel(
  dispatch: Dispatch<Action>,
  ch: ChannelDto,
  /** Where a failed open should leave the user: whichever view they clicked
   *  from, not an arbitrary one. */
  back: Mode = "browse",
) {
  // Separators are inert by construction; nothing should be able to open one.
  if (ch.isSeparator) return;

  // Metadata paints immediately — name, logo, programme — while the stream
  // opens, which is the whole trick for perceived latency.
  dispatch({ type: "opening", channel: ch });
  const ok = await run(dispatch, `play ${ch.cleanName}`, backend.play(ch.streamId));
  // A failed open should not strand the user staring at nothing: put them
  // back where they came from, with the toast explaining why.
  if (!ok) dispatch({ type: "mode", mode: back });
}

// ------------------------------------------------------------------ actions

/** What an action needs beyond the dispatcher: only the player, and only
 *  because "toggle compressor" has to read the current value to flip it. */
export interface ActionContext {
  dispatch: Dispatch<Action>;
  player: PlayerStateDto;
}

/** One behaviour per action id. `Record<ActionId, …>` makes the compiler
 *  reject both halves of the drift: an action the palette offers that nothing
 *  performs, and a behaviour nothing can reach. The command palette and the
 *  toolbar's hamburger both come through here, so the same entry cannot mean
 *  two different things depending on where it was clicked. */
const HANDLERS: Record<ActionId, (ctx: ActionContext) => void> = {
  browse: ({ dispatch }) => dispatch({ type: "mode", mode: "browse" }),
  guide: ({ dispatch }) => dispatch({ type: "mode", mode: "guide" }),
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
  sync: ({ dispatch }) => void run(dispatch, "sync", backend.syncNow()),
  settings: ({ dispatch }) => dispatch({ type: "settings", open: true }),
  about: ({ dispatch }) => dispatch({ type: "about", open: true }),
};

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
