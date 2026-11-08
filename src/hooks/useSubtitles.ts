// Subtitles on or off, from the picture's menu, the transport's toggle and
// C: one way to choose, which shows the track and makes it the standing
// choice every open applies (src/logic/subtitles.ts).
//
// The backend's player takes the choice as the standing one the moment it
// is made; the settings, which outlive the app, follow through the same
// debounced save as the Settings pane's.

import { useCallback } from "react";
import { backend, capabilities } from "../backend";
import {
  afterChoosing,
  hasSubtitles,
  subtitlesShowing,
  toggleChoice,
} from "../logic/subtitles";
import { run, useApp, useDispatch } from "../store";
import type { PlayerOptionsDto, TrackDto } from "../types";
import { useSettings } from "./useSettings";

export interface Subtitles {
  /** The stream has subtitles that can be shown here: the toggle is there. */
  available: boolean;
  /** Subtitles are showing: the toggle is lit. */
  showing: boolean;
  /** Shows track `id`, or hides subtitles with `null` (the track is kept),
   *  and keeps that as the standing choice. Resolves to the options as they
   *  are after. */
  choose: (id: number | null) => Promise<PlayerOptionsDto>;
  /** Keeps `track` as the standing choice, as `choose` does: for a track
   *  that came in selected, as a downloaded file added to the player does. */
  remember: (track: Pick<TrackDto, "lang">) => void;
  /** Hides them when they show; shows the track chosen when they don't
   *  (or, with none chosen yet, the one an open would choose). */
  toggle: () => void;
}

export function useSubtitles(): Subtitles {
  const { options, settings, settingsStatus } = useApp();
  const dispatch = useDispatch();
  const { patch } = useSettings();
  const pictures = capabilities.pictureSubtitles;

  const keep = useCallback(
    (track: Pick<TrackDto, "lang"> | null) => {
      // Never saved over settings that have not been read: that would put
      // the defaults back for everything else.
      if (settingsStatus.status === "ready") patch(afterChoosing(track, settings));
    },
    [patch, settings, settingsStatus.status],
  );

  const choose = useCallback(
    (id: number | null) => {
      const track = id === null ? null : (options.subtitles.find((t) => t.id === id) ?? null);
      return backend.setSubtitleTrack(id).then((o) => {
        keep(track);
        return o;
      });
    },
    [options.subtitles, keep],
  );

  const toggle = useCallback(() => {
    const id = toggleChoice(
      options.subtitles,
      options.subtitlesVisible,
      settings.subtitleLanguage,
      pictures,
    );
    if (id === undefined) return;
    void run(
      dispatch,
      "subtitles",
      choose(id).then((value) => dispatch({ type: "options", value })),
    );
  }, [choose, dispatch, options.subtitles, options.subtitlesVisible, pictures, settings.subtitleLanguage]);

  return {
    available: hasSubtitles(options.subtitles, pictures),
    showing: subtitlesShowing(options),
    choose,
    remember: keep,
    toggle,
  };
}
