// The ⚙ modal: a left sidebar of sections, and no curation page by design.
//
// Every control writes through useSettings: the value moves at once,
// the save is debounced, and a failed save is said out loud without taking the
// user's value away.

import { useEffect, useRef, useState } from "react";
import { backend } from "../backend";
import { useSettings, type SettingsControl } from "../hooks/useSettings";
import { clampVolume, duration } from "../logic/format";
import {
  REWIND_BITRATES,
  REWIND_MAX,
  REWIND_MIN,
  REWIND_STEP,
  rewindSeconds,
  SUB_SIZE_MAX,
  SUB_SIZE_MIN,
  SYNC_MAX,
  SYNC_MIN,
  normaliseColor,
  subtitlePreview,
} from "../logic/settings";
import { MAX_GUIDE_HOURS, MIN_GUIDE_HOURS } from "../logic/guide";
import { MOUSE_WORDS, SHORTCUT_GROUPS } from "../logic/shortcuts";
import { VOLUME_MAX } from "../logic/volume";
import { kindLabel, statusLine as updateStatusLine } from "../logic/updates";
import { checkForUpdates } from "../actions";
import { run, useApp, useDispatch } from "../store";
import { AboutPane } from "./AboutPane";
import { AccountPane } from "./AccountPane";
import type { SubtitleBackdrop } from "../types";

const SECTIONS = [
  "Account",
  "Playback",
  "Audio",
  "Subtitles",
  "Guide",
  "Data",
  "Updates",
  "Keyboard",
  "About",
] as const;

type Section = (typeof SECTIONS)[number];

const FONTS = ["Sans", "Serif", "Monospace"];

/** Presets rather than <input type="color">: the native swatch is a light
 *  button that opens an OS picker, and these are the four colours anyone
 *  actually uses for subtitles. The hex field is there for the rest. */
const COLORS = ["#ffffff", "#f2e600", "#00e5ff", "#c8c8c8"];

const BACKDROPS: { id: SubtitleBackdrop; label: string; note: string }[] = [
  { id: "background", label: "Background", note: "a box behind each line" },
  { id: "window", label: "Window", note: "a band across the whole line" },
  { id: "outline", label: "Outline", note: "an edge around the letters" },
  { id: "shadow", label: "Shadow", note: "offset behind the letters" },
  { id: "none", label: "None", note: "letters only" },
];

export function SettingsModal() {
  const { player, sync } = useApp();
  const dispatch = useDispatch();
  const [section, setSection] = useState<Section>("Account");
  const s = useSettings(true);
  const close = () => dispatch({ type: "settings", open: false });

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) close();
      }}
    >
      <div className="settings" role="dialog" aria-label="Settings">
        {/* Clicking outside and Escape both close this, and neither shows
            itself: a dialog with no visible way out reads as one you are
            stuck in. */}
        <button
          className="modal-close"
          aria-label="Close settings"
          data-tip="Close (Escape)"
          onClick={close}
        >
          <svg viewBox="0 0 12 12" aria-hidden="true">
            <path d="M2.5 2.5l7 7M9.5 2.5l-7 7" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
        </button>
        <nav className="settings-side">
          {SECTIONS.map((x) => (
            <button
              key={x}
              className={x === section ? "sel" : ""}
              onClick={() => setSection(x)}
            >
              {x}
            </button>
          ))}
          <SaveIndicator saving={s.saving} failed={s.saveError !== null} />
        </nav>

        <div className="settings-body">
          <h3>{section}</h3>

          {/* One line, at the top of every pane that writes settings, saying
              what state the settings themselves are in. */}
          {section === "Playback" ||
          section === "Subtitles" ||
          section === "Guide" ||
          section === "Updates" ? (
            <Status
              status={s.status}
              saveError={s.saveError}
              onRetry={s.reload}
            />
          ) : null}

          {section === "Account" ? <AccountPane /> : null}

          {section === "Playback" ? (
            <>
              <div className="row-field">
                <label htmlFor="set-rewind">
                  Rewind buffer — {s.settings.rewindMb.toLocaleString()} MB
                </label>
                <input
                  id="set-rewind"
                  type="range"
                  min={REWIND_MIN}
                  max={REWIND_MAX}
                  step={REWIND_STEP}
                  value={s.settings.rewindMb}
                  onChange={(e) => s.patch({ rewindMb: Number(e.target.value) })}
                />
              </div>
              <RewindEstimates
                megabytes={s.settings.rewindMb}
                playing={player.state === "playing" || player.state === "paused"}
              />
              <div className="row-field">
                <label htmlFor="set-hwdec">Hardware decode</label>
                <button
                  id="set-hwdec"
                  className={`ctl ${s.settings.hardwareDecode ? "on" : ""}`}
                  aria-pressed={s.settings.hardwareDecode}
                  onClick={() => s.patch({ hardwareDecode: !s.settings.hardwareDecode })}
                >
                  {s.settings.hardwareDecode ? "on" : "off"}
                </button>
              </div>
              <div className="row-field">
                <label htmlFor="set-hdr">Tone-map HDR to SDR</label>
                <button
                  id="set-hdr"
                  className={`ctl ${s.settings.toneMapHdr ? "on" : ""}`}
                  aria-pressed={s.settings.toneMapHdr}
                  onClick={() => s.patch({ toneMapHdr: !s.settings.toneMapHdr })}
                >
                  {s.settings.toneMapHdr ? "on" : "off"}
                </button>
              </div>
              <p>
                The rewind buffer is memory set aside for live scrollback.
                How far back it reaches depends on the channel’s bitrate, so
                the times above are estimates. Turning tone mapping off
                leaves HDR channels washed out on an SDR panel.
              </p>
            </>
          ) : null}

          {section === "Subtitles" ? (
            <>
              <Preview />

              <div className="row-field">
                <label htmlFor="set-font">Font</label>
                <select
                  id="set-font"
                  value={FONTS.includes(s.settings.subtitle.font) ? s.settings.subtitle.font : ""}
                  onChange={(e) => s.patchSubtitle({ font: e.target.value })}
                >
                  {FONTS.map((f) => (
                    <option key={f} value={f}>
                      {f}
                    </option>
                  ))}
                  {FONTS.includes(s.settings.subtitle.font) ? null : (
                    <option value="">{s.settings.subtitle.font}</option>
                  )}
                </select>
              </div>

              <div className="row-field">
                <label htmlFor="set-size">Size — {s.settings.subtitle.size}pt at 720p</label>
                <input
                  id="set-size"
                  type="range"
                  min={SUB_SIZE_MIN}
                  max={SUB_SIZE_MAX}
                  step={1}
                  value={s.settings.subtitle.size}
                  onChange={(e) => s.patchSubtitle({ size: Number(e.target.value) })}
                />
              </div>

              <div className="row-field">
                <label>Colour</label>
                <div className="swatches">
                  {COLORS.map((c) => (
                    <button
                      key={c}
                      className={`swatch ${s.settings.subtitle.color === c ? "sel" : ""}`}
                      style={{ background: c }}
                      data-tip={c}
                      aria-label={c}
                      onClick={() => s.patchSubtitle({ color: c })}
                    />
                  ))}
                  <input
                    className="field hex"
                    value={s.settings.subtitle.color}
                    spellCheck={false}
                    aria-label="Subtitle colour, hex"
                    onChange={(e) => s.patchSubtitle({ color: normaliseColor(e.target.value) })}
                  />
                </div>
              </div>

              <div className="row-field wrap">
                <label>Backdrop</label>
                <div className="choices">
                  {BACKDROPS.map((b) => (
                    <button
                      key={b.id}
                      className={`ctl ${s.settings.subtitle.backdrop === b.id ? "on" : ""}`}
                      data-tip={b.note}
                      onClick={() => s.patchSubtitle({ backdrop: b.id })}
                    >
                      {b.label}
                    </button>
                  ))}
                </div>
              </div>

              {s.settings.subtitle.backdrop === "background" ||
              s.settings.subtitle.backdrop === "window" ? (
                <div className="row-field">
                  <label htmlFor="set-op">
                    Backdrop opacity — {Math.round(s.settings.subtitle.backdropOpacity * 100)}%
                  </label>
                  <input
                    id="set-op"
                    type="range"
                    min={0}
                    max={100}
                    step={5}
                    value={Math.round(s.settings.subtitle.backdropOpacity * 100)}
                    onChange={(e) =>
                      s.patchSubtitle({ backdropOpacity: Number(e.target.value) / 100 })
                    }
                  />
                </div>
              ) : null}

              {s.settings.subtitle.backdrop === "outline" ? (
                <div className="row-field">
                  <label htmlFor="set-outline">
                    Outline size — {s.settings.subtitle.outlineSize}
                  </label>
                  <input
                    id="set-outline"
                    type="range"
                    min={0}
                    max={8}
                    step={0.5}
                    value={s.settings.subtitle.outlineSize}
                    onChange={(e) => s.patchSubtitle({ outlineSize: Number(e.target.value) })}
                  />
                </div>
              ) : null}

              {s.settings.subtitle.backdrop === "shadow" ? (
                <div className="row-field">
                  <label htmlFor="set-shadow">
                    Shadow offset — {s.settings.subtitle.shadowOffset}
                  </label>
                  <input
                    id="set-shadow"
                    type="range"
                    min={0}
                    max={8}
                    step={0.5}
                    value={s.settings.subtitle.shadowOffset}
                    onChange={(e) => s.patchSubtitle({ shadowOffset: Number(e.target.value) })}
                  />
                </div>
              ) : null}

              <p>
                The real subtitles are drawn by libass into the video plane,
                underneath this window, so they cannot be shown here. The
                preview is CSS standing in for them: the size scales the same
                way mpv scales it — points at 720p — and each backdrop is the
                shape mpv will draw.
              </p>
            </>
          ) : null}

          {section === "Guide" ? (
            <>
              <div className="row-field">
                <label htmlFor="set-hours">
                  Hours per screen — {s.settings.guideHours}
                </label>
                <input
                  id="set-hours"
                  type="range"
                  min={MIN_GUIDE_HOURS}
                  max={MAX_GUIDE_HOURS}
                  step={1}
                  value={s.settings.guideHours}
                  onChange={(e) => s.patch({ guideHours: Number(e.target.value) })}
                />
              </div>
              <div className="row-field">
                <label htmlFor="set-sync">
                  Refresh the catalogue every {s.settings.syncIntervalHours}h
                </label>
                <input
                  id="set-sync"
                  type="range"
                  min={SYNC_MIN}
                  max={SYNC_MAX}
                  step={1}
                  value={s.settings.syncIntervalHours}
                  onChange={(e) => s.patch({ syncIntervalHours: Number(e.target.value) })}
                />
              </div>
              <p>
                Fewer hours per screen means wider programme blocks and less
                squinting; more means less paging. The guide reads this
                immediately — press <b>G</b> to see it.
              </p>
            </>
          ) : null}

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
                compressor is ffmpeg’s <code>speechnorm</code> followed by a
                limiter.
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

          {section === "Updates" ? <UpdatesPane s={s} /> : null}

          {section === "Keyboard" ? <KeyboardPane /> : null}

          {section === "About" ? <AboutPane /> : null}
        </div>
      </div>
    </div>
  );
}

/** Every shortcut and gesture, drawn from SHORTCUT_GROUPS: keys as keycaps,
 *  what they do beside them, grouped by where they apply. */
function KeyboardPane() {
  return (
    <div className="kb">
      {SHORTCUT_GROUPS.map((g) => (
        <section key={g.title} className="kb-group">
          <h4>
            {g.title}
            {g.when ? <span className="kb-when">{g.when}</span> : null}
          </h4>
          <dl className="kb-list">
            {g.items.map((it) => (
              <div key={`${it.does}`} className="kb-row">
                <dt>
                  {it.keys.map((chord, i) => (
                    <span key={chord.join("+")} className="kb-alt">
                      {i > 0 ? <span className="kb-sep">{it.pair ? "/" : "or"}</span> : null}
                      {chord.map((k, j) => (
                        <span key={k} className="kb-chord">
                          {j > 0 ? <span className="kb-plus">+</span> : null}
                          <kbd className={MOUSE_WORDS.has(k) ? "mouse" : ""}>{k}</kbd>
                        </span>
                      ))}
                    </span>
                  ))}
                </dt>
                <dd>{it.does}</dd>
              </div>
            ))}
          </dl>
        </section>
      ))}
    </div>
  );
}

/** The automatic check, and the way to ask now. The setting is an ordinary
 *  one, saved like the rest; the check itself is the update dialog's, so
 *  "Check now" opens it over this pane rather than answering in a line here
 *  that would then need its own download and install buttons. */
function UpdatesPane({ s }: { s: SettingsControl }) {
  const { update } = useApp();
  const dispatch = useDispatch();
  const configured = update.state.state !== "notConfigured";

  return (
    <>
      <div className={`settings-note ${update.state.state === "failed" ? "bad" : ""}`}>
        {updateStatusLine(update)}
      </div>
      <div className="row-field">
        <label htmlFor="set-autoupdate">Check for updates automatically</label>
        <button
          id="set-autoupdate"
          className={`ctl ${s.settings.checkForUpdates ? "on" : ""}`}
          aria-pressed={s.settings.checkForUpdates}
          onClick={() => s.patch({ checkForUpdates: !s.settings.checkForUpdates })}
        >
          {s.settings.checkForUpdates ? "on" : "off"}
        </button>
      </div>
      <div className="row-field">
        <label>
          Installed: {update.current || "unknown"} · {kindLabel(update.kind)}
        </label>
        <button
          className="ctl"
          disabled={!configured}
          onClick={() => {
            dispatch({ type: "updateDialog", open: true });
            void checkForUpdates(dispatch);
          }}
        >
          Check now
        </button>
      </div>
      <p>
        A minute after zapper starts, and once a day while it runs, it asks
        GitHub for its list of releases. Nothing about you, your provider or what
        you watch is sent — only zapper’s version, in the request’s user agent.
        An update is downloaded only when you choose to install it, and only
        installed if it matches the checksum published with the release.
      </p>
    </>
  );
}

function Status({
  status,
  saveError,
  onRetry,
}: {
  status: ReturnType<typeof useSettings>["status"];
  saveError: string | null;
  onRetry: () => void;
}) {
  return (
    <>
      {status.status === "error" ? (
        <div className="settings-note bad">
          <span>
            Settings could not be read — {status.message}. These are the
            defaults; changes will not stick until the backend answers.
          </span>
          <button className="link" onClick={onRetry}>
            try again
          </button>
        </div>
      ) : null}
      {saveError ? (
        <div className="settings-note bad">
          <span>Not saved — {saveError}. Your value is still here.</span>
        </div>
      ) : null}
    </>
  );
}

/** What the rewind buffer buys, in time, at bitrates people recognise — and
 *  at the channel playing now, measured, which is the one that matters.
 *
 *  The setting is megabytes because that is all mpv's buffer understands;
 *  saying "4 minutes" instead was a promise it kept only at one bitrate. */
function RewindEstimates({ megabytes, playing }: { megabytes: number; playing: boolean }) {
  const [channelMbps, setChannelMbps] = useState<number | null>(null);
  useEffect(() => {
    if (!playing) {
      setChannelMbps(null);
      return;
    }
    let live = true;
    void backend
      .playerStats()
      .then((st) => {
        const bps = (st.videoBitrate ?? 0) + (st.audioBitrate ?? 0);
        if (live) setChannelMbps(bps > 0 ? bps / 1_000_000 : null);
      })
      .catch(() => {
        // The common bitrates below still answer the question.
      });
    return () => {
      live = false;
    };
  }, [playing]);

  return (
    <dl className="rewind-estimates" aria-label="How far back the buffer reaches">
      {channelMbps !== null ? (
        <div className="this-channel">
          <dt>This channel</dt>
          <dd className="rate">{channelMbps.toFixed(1)} Mbps</dd>
          <dd>{duration(rewindSeconds(megabytes, channelMbps))}</dd>
        </div>
      ) : null}
      {REWIND_BITRATES.map((b) => (
        <div key={b.label}>
          <dt>{b.label}</dt>
          <dd className="rate">{b.mbps} Mbps</dd>
          <dd>{duration(rewindSeconds(megabytes, b.mbps))}</dd>
        </div>
      ))}
    </dl>
  );
}

/** How long "Saved" stays after a save lands. */
const SAVED_FOR_MS = 1500;

/** The routine half of the save status, in the corner of the section list.
 *
 *  A box at the top of the pane would appear and vanish on every toggle and
 *  push everything under it down and back up — the control just clicked
 *  would move out from under the pointer. Down here it sits
 *  in space nothing else uses, so it can come and go freely. Failures are
 *  not routine and stay at the top of the pane, where `Status` puts them. */
function SaveIndicator({ saving, failed }: { saving: boolean; failed: boolean }) {
  const [saved, setSaved] = useState(false);
  const was = useRef(saving);
  useEffect(() => {
    const finished = was.current && !saving;
    was.current = saving;
    if (saving || !finished || failed) {
      setSaved(false);
      return;
    }
    setSaved(true);
    const t = window.setTimeout(() => setSaved(false), SAVED_FOR_MS);
    return () => window.clearTimeout(t);
  }, [saving, failed]);

  const text = saving ? "Saving…" : saved ? "Saved" : "";
  return (
    <div className={`settings-saving ${text ? "shown" : ""}`} role="status" aria-live="polite">
      {text}
    </div>
  );
}

/** A sample frame with the subtitle drawn the way mpv will draw it.
 *
 *  The frame's own height is measured rather than assumed, because the size is
 *  points at 720p and the only honest way to show it is to scale by how tall
 *  this stand-in frame actually is. */
function Preview() {
  const { settings } = useApp();
  const ref = useRef<HTMLDivElement | null>(null);
  const [height, setHeight] = useState(240);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const read = () => setHeight(el.clientHeight || 240);
    read();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(read);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const p = subtitlePreview(settings.subtitle, height);

  return (
    <div className="sub-preview" ref={ref}>
      <div className="sub-frame" aria-hidden />
      <div className="sub-line" style={p.band ? { background: p.band.background } : undefined}>
        <span style={p.text}>— And that is how the whole thing started.</span>
      </div>
    </div>
  );
}
