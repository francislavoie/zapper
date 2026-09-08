// The ⚙ modal: a left sidebar of sections, and no curation page by design.
//
// Most of what belongs here is mpv properties and sync preferences, and there
// are no commands for those in the contract — `src/api.ts` exposes playback,
// catalogue and sync, nothing else. So each section says plainly what it will
// hold rather than pretending to a control that goes nowhere. The two things
// that *are* wired are wired.

import { useState } from "react";
import { backend } from "../backend";
import { run, useApp, useDispatch } from "../store";
import { clampVolume } from "../logic/format";
import { VOLUME_MAX } from "../logic/volume";

const SECTIONS = [
  "Playback",
  "Audio",
  "Subtitles",
  "Guide",
  "Data",
  "Keyboard",
  "About",
] as const;

type Section = (typeof SECTIONS)[number];

export function SettingsModal() {
  const { player, sync } = useApp();
  const dispatch = useDispatch();
  const [section, setSection] = useState<Section>("Playback");
  const close = () => dispatch({ type: "settings", open: false });

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) close();
      }}
    >
      <div className="settings" role="dialog" aria-label="Settings">
        <nav className="settings-side">
          {SECTIONS.map((s) => (
            <button
              key={s}
              className={s === section ? "sel" : ""}
              onClick={() => setSection(s)}
            >
              {s}
            </button>
          ))}
        </nav>

        <div className="settings-body">
          <h3>{section}</h3>

          {section === "Audio" ? (
            <>
              <div className="row-field">
                <label htmlFor="set-comp">Compressor</label>
                <button
                  id="set-comp"
                  className={`ctl ${player.compressor ? "on" : ""}`}
                  onClick={() =>
                    void run(
                      dispatch,
                      "compressor",
                      backend.setCompressor(!player.compressor),
                    )
                  }
                >
                  {player.compressor ? "on" : "off"}
                </button>
              </div>
              <div className="row-field">
                <label htmlFor="set-vol">Volume ({player.volume}%)</label>
                <input
                  id="set-vol"
                  type="range"
                  min={0}
                  max={VOLUME_MAX}
                  value={player.volume}
                  onChange={(e) =>
                    void run(
                      dispatch,
                      "volume",
                      backend.setVolume(clampVolume(Number(e.target.value), VOLUME_MAX)),
                    )
                  }
                />
              </div>
              <p>
                Boost above 100% is mpv’s <code>volume-max</code>; the
                compressor is <code>af=acompressor</code> with the published
                parameters.
              </p>
            </>
          ) : null}

          {section === "Data" ? (
            <>
              <div className="row-field">
                <label>
                  Last synced:{" "}
                  {sync.lastSynced
                    ? new Date(sync.lastSynced * 1000).toLocaleString()
                    : "never"}
                </label>
                <button
                  className="ctl"
                  onClick={() => void run(dispatch, "sync", backend.syncNow())}
                >
                  Refresh now
                </button>
              </div>
              <p>
                {sync.channelsTotal.toLocaleString()} channels known. Sync
                failures are non-fatal — the last good catalogue keeps serving.
              </p>
            </>
          ) : null}

          {section === "Keyboard" ? (
            <ul>
              <li>
                <b>/</b> — search channels
              </li>
              <li>
                <b>Ctrl+P</b> — command palette
              </li>
              <li>
                <b>F</b> — fullscreen · <b>Space</b> — pause · <b>←</b>/<b>→</b>{" "}
                — 5s · <b>L</b> — live edge
              </li>
              <li>
                <b>Escape</b> — back one step
              </li>
              <li>Scroll over the video adjusts volume. Clicking does not pause.</li>
            </ul>
          ) : null}

          {section === "About" ? (
            <p>
              zapper — a live TV player, 60,000 channels, one
              connection. Video is drawn by mpv beneath this window, not
              by the browser engine.
            </p>
          ) : null}

          {section === "Playback" || section === "Subtitles" || section === "Guide" ? (
            <p>
              Nothing to set here yet: these are mpv properties and sync
              preferences, and the command boundary in <code>src/api.ts</code>{" "}
              does not expose them. Rather than show a control that silently
              does nothing, this page waits for the commands to exist.
            </p>
          ) : null}
        </div>
      </div>
    </div>
  );
}
