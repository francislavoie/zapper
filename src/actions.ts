// Things more than one pane can do. Kept out of components so the list, the
// palette, the guide and the chrome all fail the same way.

import type { Dispatch } from "react";
import { backend } from "./backend";
import { catchupMinutes } from "./logic/guide";
import { run, type Action, type Mode } from "./store";
import type { ChannelDto, ProgrammeDto } from "./types";

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
