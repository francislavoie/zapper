// Loading and saving settings, with the two rules that make a settings pane
// bearable:
//
//  - a control moves the instant you move it, and the save follows, debounced;
//  - a failed save is said out loud and *keeps the user's value*. Snapping a
//    slider back to what the backend last managed to store is how you make
//    someone drag it four more times.
//
// The value itself lives in the store, because the guide reads guideHours too.

import { useCallback, useEffect, useRef, useState } from "react";
import { backend } from "../backend";
import { errorMessage, failed, loading, ready, type Async } from "../logic/async";
import { clampSettings, planSave } from "../logic/settings";
import { trackSave } from "../quit";
import { useApp, useDispatch } from "../store";
import type { SettingsDto, SubtitleStyleDto } from "../types";

/** Long enough that dragging a slider is one save, short enough that a change
 *  feels applied by the time you look away. */
const DEBOUNCE_MS = 400;

export interface SettingsControl {
  settings: SettingsDto;
  status: Async<null>;
  saveError: string | null;
  saving: boolean;
  reload: () => void;
  patch: (p: Partial<SettingsDto>) => void;
  patchSubtitle: (p: Partial<SubtitleStyleDto>) => void;
}

export function useSettings(loadOnMount = false): SettingsControl {
  const { settings, settingsStatus, settingsSaveError } = useApp();
  const dispatch = useDispatch();
  const [saving, setSaving] = useState(false);

  /** The newest value, updated before the dispatch so two changes in one tick
   *  cannot clobber each other. */
  const latest = useRef(settings);
  latest.current = settings;
  /** The last value the backend acknowledged; what a save is diffed against. */
  const saved = useRef(settings);
  const timer = useRef<number | undefined>(undefined);

  const reload = useCallback(() => {
    dispatch({ type: "settingsStatus", value: loading() });
    backend
      .settings()
      .then((s) => {
        const clean = clampSettings(s);
        saved.current = clean;
        latest.current = clean;
        dispatch({ type: "settingsValue", value: clean });
        dispatch({ type: "settingsStatus", value: ready(null) });
      })
      .catch((e: unknown) => {
        // Not fatal: the contract's defaults are already in the store, the app
        // stays usable, and the pane says so rather than showing blank rows.
        dispatch({ type: "settingsStatus", value: failed(errorMessage(e)) });
      });
  }, [dispatch]);

  const flush = useCallback(() => {
    timer.current = undefined;
    const next = latest.current;
    const plan = planSave(saved.current, next);
    setSaving(true);
    // Tracked so that quitting waits for the write itself (not for applying
    // it to the player, which a quitting app no longer needs).
    trackSave(backend.setSettings(next))
      .then(() =>
        Promise.all([
          plan.applyPlayback ? backend.applyPlaybackSettings(next) : undefined,
          plan.applySubtitle ? backend.setSubtitleStyle(next.subtitle) : undefined,
        ]),
      )
      .then(() => {
        saved.current = next;
        dispatch({ type: "settingsSaveError", message: null });
      })
      .catch((e: unknown) => {
        // The value stays exactly where the user put it.
        dispatch({ type: "settingsSaveError", message: errorMessage(e) });
      })
      .finally(() => setSaving(false));
  }, [dispatch]);

  const schedule = useCallback(() => {
    if (timer.current !== undefined) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(flush, DEBOUNCE_MS);
  }, [flush]);

  const patch = useCallback(
    (p: Partial<SettingsDto>) => {
      const value = clampSettings({ ...latest.current, ...p });
      latest.current = value;
      dispatch({ type: "settingsValue", value });
      schedule();
    },
    [dispatch, schedule],
  );

  const patchSubtitle = useCallback(
    (p: Partial<SubtitleStyleDto>) => patch({ subtitle: { ...latest.current.subtitle, ...p } }),
    [patch],
  );

  useEffect(() => {
    if (loadOnMount) reload();
    // Closing the pane, or quitting, mid-debounce must not lose the last
    // change. Quitting sends `pagehide` without unmounting anything, so the
    // page hiding has to flush too.
    const flushPending = () => {
      if (timer.current !== undefined) {
        window.clearTimeout(timer.current);
        flush();
      }
    };
    window.addEventListener("pagehide", flushPending);
    return () => {
      window.removeEventListener("pagehide", flushPending);
      flushPending();
    };
  }, [flush, loadOnMount, reload]);

  return {
    settings,
    status: settingsStatus,
    saveError: settingsSaveError,
    saving,
    reload,
    patch,
    patchSubtitle,
  };
}
