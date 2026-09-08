// Things more than one pane can do. Kept out of components so the list, the
// palette and the chrome all fail the same way.

import type { Dispatch } from "react";
import { backend } from "./backend";
import { run, type Action } from "./store";
import type { ChannelDto } from "./types";

export async function playChannel(dispatch: Dispatch<Action>, ch: ChannelDto) {
  // Separators are inert by construction; nothing should be able to open one.
  if (ch.isSeparator) return;

  // Metadata paints immediately — name, logo, programme — while the stream
  // opens, which is the whole trick for perceived latency.
  dispatch({ type: "opening", channel: ch });
  const ok = await run(dispatch, `play ${ch.cleanName}`, backend.play(ch.streamId));
  // A failed open should not strand the user staring at nothing: put them
  // back in the list they came from, with the toast explaining why.
  if (!ok) dispatch({ type: "mode", mode: "browse" });
}
