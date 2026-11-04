// The ⚙ modal: a left sidebar of pages, and each page its settings in titled
// boxes (SettingsParts.tsx). No curation page, by design.
//
// Every control writes through useSettings: the value moves at once, the
// save is debounced, and a failed save is said out loud without taking the
// user's value away. Each stored setting can be put back on its own with the
// ↺ its row shows once it differs from the default.

import { useEffect, useRef, useState } from "react";
import { backend, capabilities } from "../backend";
import { settingsSections, type SettingsSection } from "../logic/capabilities";
import { useSettings, type SettingsControl } from "../hooks/useSettings";
import { clampVolume, dateTime, duration, hhmm, systemUses12h, uses12h } from "../logic/format";
import {
  DEFAULT_SETTINGS,
  DEFAULT_SUBTITLE,
  LIVE_START_CHOICES,
  REWIND_BITRATES,
  REWIND_MAX,
  REWIND_MIN,
  REWIND_STEP,
  rewindSeconds,
  SUB_SIZE_MAX,
  SUB_SIZE_MIN,
  SYNC_MAX,
  SYNC_MIN,
  TIME_FORMAT_LABEL,
  isDefault,
  normaliseColor,
  resetPatch,
  subtitlePreview,
  type SettingKey,
} from "../logic/settings";
import { isBusy } from "../logic/toolbar.ts";
import { MAX_GUIDE_HOURS, MIN_GUIDE_HOURS } from "../logic/guide";
import { chordKeys } from "../logic/keys";
import { MOUSE_WORDS, shortcutGroups } from "../logic/shortcuts";
import { IS_MAC } from "../platform";
import { SEARCH_SYNTAX } from "../logic/searchQuery";
import { VOLUME_MAX, VOLUME_UNITY } from "../logic/volume";
import { kindLabel, statusLine as updateStatusLine } from "../logic/updates";
import { checkForUpdates } from "../actions";
import { run, useApp, useDispatch } from "../store";
import { AboutPane } from "./AboutPane";
import { AccountPane } from "./AccountPane";
import { ConnectionsPane } from "./ConnectionsPane";
import { BackupPane } from "./BackupPane";
import { RecordingPane } from "./RecordingPane";
import { RangeField, Segmented, SettingRow, SettingsGroup, SettingsPage } from "./SettingsParts";
import { Switch } from "./Switch";
import { VolumeControl } from "./VolumeControl";
import type { LiveStart, SubtitleBackdrop, SettingsDto } from "../types";

/** The pages there is something to do on here: in a browser, not the
 *  player's or updates (logic/capabilities.ts). */
const SECTIONS = settingsSections(capabilities);

type Section = SettingsSection;

const asSection = (page: string | undefined): Section | undefined =>
  SECTIONS.find((s) => s === page);

/** The line under each page's title: what is on it. */
const SUMMARY: Record<Section, string> = {
  Account: "Where the channels and the guide come from.",
  Playback: "How much live TV is kept for rewinding, how video is decoded, and the media keys.",
  Audio: "The volume and the compressor, for whatever is playing.",
  Subtitles: "How subtitles look. mpv draws them under this window; the preview stands in.",
  Guide: capabilities.data
    ? "The guide's width, how times are written, and how often the catalogue refreshes."
    : "The guide's width and how times are written, as this browser keeps them.",
  Recording: capabilities.localService
    ? "The recording service on this computer, what it does with each recording, and which other devices may reach it."
    : "The recording service this page comes from, and what it does with each recording.",
  Data: capabilities.data
    ? "The catalogue, and backups of your settings and favourites."
    : "The catalogue the recording service keeps, and refreshing it from the provider.",
  Updates: "Whether zapper looks for new versions, and which one is installed.",
  Keyboard: "Every shortcut and mouse gesture, grouped by where it works.",
  About: "",
};

const FONTS = ["Sans", "Serif", "Monospace"];

/** Presets rather than <input type="color">: the native swatch is a light
 *  button that opens an OS picker, and these are the four colours anyone
 *  actually uses for subtitles. The hex field is there for the rest. */
const COLORS = ["#ffffff", "#f2e600", "#00e5ff", "#c8c8c8"];

const BACKDROPS: { id: SubtitleBackdrop; label: string; tip: string }[] = [
  { id: "background", label: "Background", tip: "A box behind each line" },
  { id: "window", label: "Window", tip: "A band across the whole line" },
  { id: "outline", label: "Outline", tip: "An edge around the letters" },
  { id: "shadow", label: "Shadow", tip: "Offset behind the letters" },
  { id: "none", label: "None", tip: "Letters only" },
];

const BACKDROP_LABEL = Object.fromEntries(BACKDROPS.map((b) => [b.id, b.label]));

/** Pages that write stored settings, and so show their state at the top. */
const WRITES: ReadonlySet<Section> = new Set(["Playback", "Subtitles", "Guide", "Updates"]);

export function SettingsModal() {
  const dispatch = useDispatch();
  // The first page there is: Account where it can be changed, the page with
  // something to do on a first run. A caller that knows why Settings is
  // being opened can name another.
  const asked = useApp().settingsPage;
  const [section, setSection] = useState<Section>(
    () => asSection(asked?.page) ?? SECTIONS[0] ?? "About",
  );
  useEffect(() => {
    const page = asSection(asked?.page);
    if (page) setSection(page);
  }, [asked]);
  const s = useSettings(true);
  const close = () => dispatch({ type: "settings", open: false });
  const bodyRef = useRef<HTMLDivElement | null>(null);

  // A new page starts at its top, not wherever the last one was scrolled to.
  useEffect(() => {
    bodyRef.current?.scrollTo?.(0, 0);
  }, [section]);

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
        <nav className="settings-side" aria-label="Settings pages">
          <div className="settings-side-title">Settings</div>
          {SECTIONS.map((x) => (
            <button
              key={x}
              className={x === section ? "sel" : ""}
              aria-current={x === section ? "page" : undefined}
              onClick={() => setSection(x)}
            >
              {x}
            </button>
          ))}
          <SaveIndicator saving={s.saving} failed={s.saveError !== null} />
        </nav>

        <div className="settings-body" ref={bodyRef}>
          <SettingsPage heading={section} summary={SUMMARY[section]}>
            {/* One line, at the top of every page that writes settings,
                saying what state the settings themselves are in. */}
            {WRITES.has(section) ? (
              <Status status={s.status} saveError={s.saveError} onRetry={s.reload} />
            ) : null}

            {section === "Account" ? (
              <>
                <AccountPane />
                {/* How many streams the account allows is about playing
                    here, which a browser does not do. */}
                {capabilities.mpv ? <ConnectionsPane s={s} /> : null}
              </>
            ) : null}
            {section === "Playback" ? <PlaybackPage s={s} /> : null}
            {section === "Audio" ? <AudioPage /> : null}
            {section === "Subtitles" ? <SubtitlesPage s={s} /> : null}
            {section === "Guide" ? <GuidePage s={s} /> : null}
            {section === "Recording" ? <RecordingPane s={s} /> : null}
            {section === "Data" ? <DataPage s={s} /> : null}
            {section === "Updates" ? <UpdatesPage s={s} /> : null}
            {section === "Keyboard" ? <KeyboardPane /> : null}
            {section === "About" ? <AboutPane /> : null}
          </SettingsPage>
        </div>
      </div>
    </div>
  );
}

/** A row's ↺ for a stored setting: shown while it differs from the default,
 *  and putting back that one setting. */
function resettable(s: SettingsControl, key: SettingKey, resetTo: string) {
  return {
    modified: !isDefault(s.settings, key),
    onReset: () => s.patch(resetPatch(s.settings, key)),
    resetTo,
  };
}

const onOff = (b: boolean) => (b ? "on" : "off");

// ---------------------------------------------------------------- playback

function PlaybackPage({ s }: { s: SettingsControl }) {
  const { player } = useApp();
  const playing = player.state === "playing" || player.state === "paused";
  const channelMbps = useChannelMbps(playing);
  const mb = s.settings.rewindMb;
  // What the buffer buys, said at the channel playing now when it has been
  // measured, and otherwise at a bitrate most HD channels are near.
  const reach =
    channelMbps !== null
      ? `≈ ${duration(rewindSeconds(mb, channelMbps))} on this channel`
      : `≈ ${duration(rewindSeconds(mb, 10))} at 1080p`;
  const d = DEFAULT_SETTINGS;

  return (
    <>
      <SettingsGroup heading="Rewind">
        <SettingRow
          label="Rewind buffer"
          htmlFor="set-rewind"
          description="Memory set aside for live scrollback. How far back it reaches depends on the channel’s bitrate, so the times are estimates."
          {...resettable(s, "rewindMb", `${d.rewindMb} MB`)}
        >
          <RangeField
            id="set-rewind"
            min={REWIND_MIN}
            max={REWIND_MAX}
            step={REWIND_STEP}
            value={mb}
            mark={d.rewindMb}
            format={(v) => `${v.toLocaleString("en")} MB`}
            note={reach}
            onChange={(v) => s.patch({ rewindMb: v })}
          />
        </SettingRow>
        <SettingRow label="How far back that reaches" layout="stack">
          <RewindEstimates megabytes={mb} channelMbps={channelMbps} />
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup heading="Video">
        <SettingRow
          label="Hardware decode"
          htmlFor="set-hwdec"
          description="Decode on the graphics card where it can. Off decodes on the processor, which is slower but sidesteps a driver that draws garbage."
          {...resettable(s, "hardwareDecode", onOff(d.hardwareDecode))}
        >
          <Switch
            id="set-hwdec"
            checked={s.settings.hardwareDecode}
            onChange={(v) => s.patch({ hardwareDecode: v })}
          />
        </SettingRow>
        <SettingRow
          label="Tone-map HDR to SDR"
          htmlFor="set-hdr"
          description="Off leaves HDR channels washed out on an SDR panel."
          {...resettable(s, "toneMapHdr", onOff(d.toneMapHdr))}
        >
          <Switch
            id="set-hdr"
            checked={s.settings.toneMapHdr}
            onChange={(v) => s.patch({ toneMapHdr: v })}
          />
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup heading="Changing channel">
        <SettingRow
          label="When a channel opens"
          htmlFor="set-live-start"
          layout="stack"
          description={
            <>
              Providers open a channel with the last half-minute or so they have buffered,
              which starts it in the past.{" "}
              {LIVE_START_CHOICES.find((c) => c.value === s.settings.liveStart)?.note}
            </>
          }
          {...resettable(
            s,
            "liveStart",
            LIVE_START_CHOICES.find((c) => c.value === d.liveStart)?.label ?? d.liveStart,
          )}
        >
          <Segmented<LiveStart>
            id="set-live-start"
            label="When a channel opens"
            value={s.settings.liveStart}
            options={LIVE_START_CHOICES.map((c) => ({ id: c.value, label: c.short, tip: c.label }))}
            onChange={(v) => s.patch({ liveStart: v })}
          />
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup heading="Desktop">
        <SettingRow
          label="Media keys and desktop controls"
          htmlFor="set-media"
          description={
            <>
              The keyboard’s play/pause, next and previous keys and the desktop’s media
              controls drive zapper, which shows them the channel and what is on it. Next and
              previous change channel within the list on screen. Off by default: the desktop
              gives those keys to one player at a time, so turning this on can take them from a
              music player.
            </>
          }
          {...resettable(s, "mediaControls", onOff(d.mediaControls))}
        >
          <Switch
            id="set-media"
            checked={s.settings.mediaControls}
            onChange={(v) => s.patch({ mediaControls: v })}
          />
        </SettingRow>
      </SettingsGroup>
    </>
  );
}

/** The bitrate of the channel playing now, measured, or null when nothing
 *  is playing or it has not been measured yet. */
function useChannelMbps(playing: boolean): number | null {
  const [mbps, setMbps] = useState<number | null>(null);
  useEffect(() => {
    if (!playing) {
      setMbps(null);
      return;
    }
    let live = true;
    void backend
      .playerStats()
      .then((st) => {
        const bps = (st.videoBitrate ?? 0) + (st.audioBitrate ?? 0);
        if (live) setMbps(bps > 0 ? bps / 1_000_000 : null);
      })
      .catch(() => {
        // The common bitrates still answer the question.
      });
    return () => {
      live = false;
    };
  }, [playing]);
  return mbps;
}

/** What the rewind buffer buys, in time, at bitrates people recognise — and
 *  at the channel playing now, measured, which is the one that matters.
 *
 *  The setting is megabytes because that is all mpv's buffer understands;
 *  saying "4 minutes" instead was a promise it kept only at one bitrate. */
function RewindEstimates({ megabytes, channelMbps }: { megabytes: number; channelMbps: number | null }) {
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

// ------------------------------------------------------------------- audio

/** The volume and the compressor are the player's, not stored settings: they
 *  start at 100% and off with every launch, so those are what ↺ puts back. */
function AudioPage() {
  const { player } = useApp();
  const dispatch = useDispatch();
  // As the player's bar does it: reaching for the slider while muted means
  // "let me hear this".
  const setVolume = (v: number) => {
    void run(dispatch, "volume", backend.setVolume(clampVolume(v, VOLUME_MAX)));
    if (player.muted) void run(dispatch, "mute", backend.setMuted(false));
  };
  const setCompressor = (on: boolean) =>
    void run(dispatch, "compressor", backend.setCompressor(on));

  return (
    <SettingsGroup heading="Sound">
      <SettingRow
        label="Volume"
        description={
          <>
            Above {VOLUME_UNITY}% is software gain, up to {VOLUME_MAX}% (mpv’s{" "}
            <code>volume-max</code>): the hatched end of the track. The same control as the
            player’s bar.
          </>
        }
        modified={player.volume !== VOLUME_UNITY}
        onReset={() => setVolume(VOLUME_UNITY)}
        resetTo={`${VOLUME_UNITY}%`}
      >
        <VolumeControl
          volume={player.volume}
          muted={player.muted}
          onVolume={setVolume}
          onToggleMute={() => void run(dispatch, "mute", backend.setMuted(!player.muted))}
          scale
        />
      </SettingRow>
      <SettingRow
        label="Compressor"
        htmlFor="set-comp"
        description={
          <>
            Evens out loud and quiet passages, for speech over a noisy room: ffmpeg’s{" "}
            <code>speechnorm</code> followed by a limiter.
          </>
        }
        modified={player.compressor}
        onReset={() => setCompressor(false)}
        resetTo="off"
      >
        <Switch id="set-comp" checked={player.compressor} onChange={setCompressor} />
      </SettingRow>
    </SettingsGroup>
  );
}

// --------------------------------------------------------------- subtitles

function SubtitlesPage({ s }: { s: SettingsControl }) {
  const sub = s.settings.subtitle;
  const d = DEFAULT_SUBTITLE;
  const customFont = !FONTS.includes(sub.font);
  return (
    <>
      <SettingsGroup heading="Preview" bare>
        <div className="set-card sub-stage">
          <Preview />
        </div>
      </SettingsGroup>

      <SettingsGroup heading="Text">
        <SettingRow label="Font" htmlFor="set-font" {...resettable(s, "subtitle.font", d.font)}>
          <select
            id="set-font"
            value={customFont ? "" : sub.font}
            onChange={(e) => s.patchSubtitle({ font: e.target.value })}
          >
            {FONTS.map((f) => (
              <option key={f} value={f}>
                {f}
              </option>
            ))}
            {customFont ? <option value="">{sub.font}</option> : null}
          </select>
        </SettingRow>
        <SettingRow
          label="Size"
          htmlFor="set-size"
          description="In points at 720 lines, the way mpv scales it to the window."
          {...resettable(s, "subtitle.size", `${d.size} pt`)}
        >
          <RangeField
            id="set-size"
            min={SUB_SIZE_MIN}
            max={SUB_SIZE_MAX}
            step={1}
            value={sub.size}
            mark={d.size}
            format={(v) => `${v} pt`}
            note="at 720p"
            onChange={(v) => s.patchSubtitle({ size: v })}
          />
        </SettingRow>
        <SettingRow label="Colour" {...resettable(s, "subtitle.color", d.color)}>
          <div className="swatches">
            {COLORS.map((c) => (
              <button
                key={c}
                className={`swatch ${sub.color === c ? "sel" : ""}`}
                style={{ background: c }}
                data-tip={c}
                aria-label={c}
                aria-pressed={sub.color === c}
                onClick={() => s.patchSubtitle({ color: c })}
              />
            ))}
            <input
              className="field hex"
              value={sub.color}
              spellCheck={false}
              aria-label="Subtitle colour, hex"
              onChange={(e) => s.patchSubtitle({ color: normaliseColor(e.target.value) })}
            />
          </div>
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup heading="Backdrop">
        <SettingRow
          label="Backdrop"
          description="What stands between the letters and the picture."
          {...resettable(s, "subtitle.backdrop", BACKDROP_LABEL[d.backdrop])}
        >
          <Segmented
            label="Backdrop"
            value={sub.backdrop}
            options={BACKDROPS}
            onChange={(backdrop) => s.patchSubtitle({ backdrop })}
          />
        </SettingRow>

        {sub.backdrop === "background" || sub.backdrop === "window" ? (
          <SettingRow
            label="Opacity"
            htmlFor="set-op"
            {...resettable(s, "subtitle.backdropOpacity", `${Math.round(d.backdropOpacity * 100)}%`)}
          >
            <RangeField
              id="set-op"
              min={0}
              max={100}
              step={5}
              value={Math.round(sub.backdropOpacity * 100)}
              mark={Math.round(d.backdropOpacity * 100)}
              format={(v) => `${v}%`}
              onChange={(v) => s.patchSubtitle({ backdropOpacity: v / 100 })}
            />
          </SettingRow>
        ) : null}

        {sub.backdrop === "outline" ? (
          <SettingRow
            label="Outline size"
            htmlFor="set-outline"
            {...resettable(s, "subtitle.outlineSize", `${d.outlineSize} px`)}
          >
            <RangeField
              id="set-outline"
              min={0}
              max={8}
              step={0.5}
              value={sub.outlineSize}
              mark={d.outlineSize}
              format={(v) => `${v} px`}
              onChange={(v) => s.patchSubtitle({ outlineSize: v })}
            />
          </SettingRow>
        ) : null}

        {sub.backdrop === "shadow" ? (
          <SettingRow
            label="Shadow offset"
            htmlFor="set-shadow"
            {...resettable(s, "subtitle.shadowOffset", `${d.shadowOffset} px`)}
          >
            <RangeField
              id="set-shadow"
              min={0}
              max={8}
              step={0.5}
              value={sub.shadowOffset}
              mark={d.shadowOffset}
              format={(v) => `${v} px`}
              onChange={(v) => s.patchSubtitle({ shadowOffset: v })}
            />
          </SettingRow>
        ) : null}
      </SettingsGroup>

      <p className="set-foot">
        The real subtitles are drawn by libass into the video plane, underneath this window, so
        they cannot be shown here. The preview is CSS standing in for them: the size scales the
        same way mpv scales it — points at 720p — and each backdrop is the shape mpv will draw.
      </p>
    </>
  );
}

// ------------------------------------------------------------------- guide

function GuidePage({ s }: { s: SettingsControl }) {
  const d = DEFAULT_SETTINGS;
  const system12 = systemUses12h();
  // 20:30 today, as an example of each way of writing it.
  const sample = new Date();
  sample.setHours(20, 30, 0, 0);
  const at = sample.getTime() / 1000;
  const shown = hhmm(at, uses12h(s.settings.timeFormat, system12));
  const formats: { id: SettingsDto["timeFormat"]; label: string; tip: string }[] = [
    { id: "auto", label: TIME_FORMAT_LABEL.auto, tip: `As the system writes it: ${hhmm(at, system12)}` },
    { id: "12h", label: TIME_FORMAT_LABEL["12h"], tip: hhmm(at, true) },
    { id: "24h", label: TIME_FORMAT_LABEL["24h"], tip: hhmm(at, false) },
  ];

  return (
    <>
      <SettingsGroup heading="Guide">
        <SettingRow
          label="Hours per screen"
          htmlFor="set-hours"
          description={
            <>
              Fewer hours means wider programme blocks and less squinting; more means less
              paging. The guide reads this at once — press <kbd>G</kbd> to see it.
            </>
          }
          {...resettable(s, "guideHours", `${d.guideHours} h`)}
        >
          <RangeField
            id="set-hours"
            min={MIN_GUIDE_HOURS}
            max={MAX_GUIDE_HOURS}
            step={1}
            value={s.settings.guideHours}
            mark={d.guideHours}
            format={(v) => `${v} h`}
            onChange={(v) => s.patch({ guideHours: v })}
          />
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup heading="Clock">
        <SettingRow
          label="Times of day"
          description={
            <>
              Every time zapper shows — the guide, now and next, programme search, the sleep
              timer — written as <b data-selectable>{shown}</b>. System follows the language
              and region settings, which write {hhmm(at, system12)}.
            </>
          }
          {...resettable(s, "timeFormat", TIME_FORMAT_LABEL[d.timeFormat])}
        >
          <Segmented
            label="Times of day"
            value={s.settings.timeFormat}
            options={formats}
            onChange={(timeFormat) => s.patch({ timeFormat })}
          />
        </SettingRow>
      </SettingsGroup>

      {/* How often the catalogue refreshes is the app's to say: a browser
          only reads it. */}
      {capabilities.data ? (
        <SettingsGroup heading="Catalogue">
          <SettingRow
            label="Refresh the catalogue"
            htmlFor="set-sync"
            description="How often the channel list and the guide are fetched again in the background."
            {...resettable(s, "syncIntervalHours", `every ${d.syncIntervalHours} h`)}
          >
            <RangeField
              id="set-sync"
              min={SYNC_MIN}
              max={SYNC_MAX}
              step={1}
              value={s.settings.syncIntervalHours}
              mark={d.syncIntervalHours}
              format={(v) => `${v} h`}
              onChange={(v) => s.patch({ syncIntervalHours: v })}
            />
          </SettingRow>
        </SettingsGroup>
      ) : null}
    </>
  );
}

// -------------------------------------------------------------------- data

function DataPage({ s }: { s: SettingsControl }) {
  const { sync } = useApp();
  const dispatch = useDispatch();
  const syncing = isBusy(sync.phase);
  return (
    <>
      <SettingsGroup heading="Catalogue">
        <SettingRow
          label={sync.lastSynced ? `Last synced ${dateTime(sync.lastSynced)}` : "Never synced"}
          description={
            <>
              {sync.channelsTotal.toLocaleString()} channels known. A failed sync is not fatal:
              the last good catalogue keeps serving.
            </>
          }
        >
          {/* Busy while a pass runs, here or (in a browser) on the service:
              the toolbar says how far it has got. */}
          <button
            className="ctl"
            disabled={syncing}
            onClick={() => void run(dispatch, "sync", backend.syncNow())}
          >
            {syncing ? "Refreshing…" : "Refresh now"}
          </button>
        </SettingRow>
      </SettingsGroup>
      {/* Backups go through this computer's file dialogs. */}
      {capabilities.data ? <BackupPane s={s} /> : null}
    </>
  );
}

// ----------------------------------------------------------------- updates

/** The automatic check, and the way to ask now. The setting is an ordinary
 *  one, saved like the rest; the check itself is the update dialog's, so
 *  "Check now" opens it over this page rather than answering in a line here
 *  that would then need its own download and install buttons. */
function UpdatesPage({ s }: { s: SettingsControl }) {
  const { update } = useApp();
  const dispatch = useDispatch();
  const configured = update.state.state !== "notConfigured";

  return (
    <>
      <div className={`settings-note ${update.state.state === "failed" ? "bad" : ""}`}>
        {updateStatusLine(update)}
      </div>
      <SettingsGroup heading="Updates">
        <SettingRow
          label="Check for updates automatically"
          htmlFor="set-autoupdate"
          description="A minute after zapper starts, and once a day while it runs. Off: only Check now asks."
          {...resettable(s, "checkForUpdates", onOff(DEFAULT_SETTINGS.checkForUpdates))}
        >
          <Switch
            id="set-autoupdate"
            checked={s.settings.checkForUpdates}
            onChange={(v) => s.patch({ checkForUpdates: v })}
          />
        </SettingRow>
        <SettingRow
          label={`Installed: ${update.current || "unknown"}`}
          description={kindLabel(update.kind)}
        >
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
        </SettingRow>
      </SettingsGroup>
      <p className="set-foot">
        The check asks GitHub for zapper’s list of releases. Nothing about you, your provider or
        what you watch is sent — only zapper’s version, in the request’s user agent. An update is
        downloaded only when you choose to install it, and only installed if it matches the
        checksum published with the release.
      </p>
    </>
  );
}

// ---------------------------------------------------------------- keyboard

/** Every shortcut and gesture that does something here, drawn from
 *  SHORTCUT_GROUPS: keys as keycaps, what they do beside them, a box per
 *  place they apply. */
const KEY_GROUPS = shortcutGroups(capabilities);

function KeyboardPane() {
  return (
    <div className="kb">
      {KEY_GROUPS.map((g) => (
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
                      {chordKeys(chord, IS_MAC).map((k, j) => (
                        <span key={k} className="kb-chord">
                          {/* A Mac writes ⌘ P with nothing between. */}
                          {j > 0 && !IS_MAC ? <span className="kb-plus">+</span> : null}
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
      {/* Not keys, but where someone looks for how searching works: the `/`
          and Ctrl+P rows above lead here. */}
      <section className="kb-group">
        <h4>
          Searching
          <span className="kb-when">channels, programmes and the palette</span>
        </h4>
        <dl className="kb-list">
          {SEARCH_SYNTAX.map((s) => (
            <div key={s.example} className="kb-row">
              <dt>
                <code className="kb-query">{s.example}</code>
              </dt>
              <dd>{s.finds}</dd>
            </div>
          ))}
        </dl>
      </section>
    </div>
  );
}

// ------------------------------------------------------------------ status

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

/** How long "Saved" stays after a save lands. */
const SAVED_FOR_MS = 1500;

/** The routine half of the save status, in the corner of the page list.
 *
 *  A box at the top of the page would appear and vanish on every toggle and
 *  push everything under it down and back up — the control just clicked
 *  would move out from under the pointer. Down here it sits in space
 *  nothing else uses, so it can come and go freely. Failures are not routine
 *  and stay at the top of the page, where `Status` puts them. */
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
