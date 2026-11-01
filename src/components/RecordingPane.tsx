// Settings › Recording: the recording service on this computer (install,
// remove, whether it keeps running after logging out), what it does with
// every recording (where, what its files are called, how much padding, the
// clock track), and whether other devices may reach it (NetworkAccess).
//
// The settings are the service's own, read and written through its API, so
// they can only be changed while it is running — all but "Simultaneous
// streams", which is the app's (Settings › Account has it too) and which the
// service reads from the catalogue the two share. Like the rest of Settings, a
// change shows at once and is saved a moment later, and each one that
// differs from the service's default has a ↺. Installing the service is the
// person's decision: nothing here runs until a button is pressed.

import { useCallback, useEffect, useRef, useState } from "react";
import { refreshRecordings } from "../actions";
import { backend } from "../backend";
import { errorMessage } from "../logic/async";
import { presetLabel } from "../logic/naming";
import { RECORDING_DEFAULTS, bytes, padMinutes, padSeconds } from "../logic/recordings";
import { DEFAULT_SETTINGS, STREAMS_MAX, STREAMS_MIN } from "../logic/settings";
import type { SettingsControl } from "../hooks/useSettings";
import { useDispatch } from "../store";
import type { RecordingSettingsDto, RecordingStatusDto, ServiceStatusDto } from "../types";
import { NamingField } from "./NamingField";
import { NetworkAccess } from "./NetworkAccess";
import { RangeField, SettingRow, SettingsGroup } from "./SettingsParts";
import { Switch } from "./Switch";

const SAVE_AFTER_MS = 400;

export function RecordingPane({ s }: { s: SettingsControl }) {
  const streams = s.settings.simultaneousStreams;
  const dStreams = DEFAULT_SETTINGS.simultaneousStreams;
  const dispatch = useDispatch();
  const [service, setService] = useState<ServiceStatusDto | null>(null);
  const [status, setStatus] = useState<RecordingStatusDto | null>(null);
  const [settings, setSettings] = useState<RecordingSettingsDto | null>(null);
  /** The folder as typed, while it is being edited: the field shows this
   *  rather than the service's value, so the refresh below cannot replace
   *  what is being typed. Null when not editing. */
  const [folderDraft, setFolderDraft] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [linger, setLinger] = useState(false);
  /** A change not yet sent: the polls below must not put the old value back
   *  under the slider while it is being dragged. */
  const pending = useRef<RecordingSettingsDto | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const load = useCallback(async () => {
    try {
      const s = await backend.recordingService();
      setService(s);
      setLinger(s.linger);
    } catch (e) {
      setError(errorMessage(e));
    }
    try {
      const [st, set] = await Promise.all([backend.recordingStatus(), backend.recordingSettings()]);
      setStatus(st);
      if (!pending.current) setSettings(set);
    } catch {
      setStatus(null);
      setSettings(null);
    }
  }, []);

  useEffect(() => {
    void load();
    // A freshly installed service takes a moment to answer; keep looking
    // while the page is open.
    const id = setInterval(() => void load(), 5_000);
    return () => clearInterval(id);
  }, [load]);

  const flush = useCallback(async () => {
    const next = pending.current;
    if (!next) return;
    try {
      const saved = await backend.setRecordingSettings(next);
      if (pending.current === next) {
        pending.current = null;
        setSettings(saved);
      }
      setError(null);
    } catch (e) {
      if (pending.current === next) pending.current = null;
      setError(errorMessage(e));
      void load();
    }
  }, [load]);

  // A page closed with a change still waiting sends it on the way out.
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
      void flush();
    },
    [flush],
  );

  const patch = (p: Partial<RecordingSettingsDto>) => {
    if (!settings) return;
    const next = { ...(pending.current ?? settings), ...p };
    pending.current = next;
    setSettings(next);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => void flush(), SAVE_AFTER_MS);
  };

  const commitFolder = () => {
    if (folderDraft === null) return;
    if (settings && folderDraft !== settings.outputDir) patch({ outputDir: folderDraft });
    setFolderDraft(null);
  };

  const act = async (what: () => Promise<ServiceStatusDto>) => {
    setBusy(true);
    setError(null);
    try {
      setService(await what());
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
      void load();
      void refreshRecordings(dispatch);
    }
  };

  const running = status !== null;
  const stateWord = !service
    ? "…"
    : !service.supported
      ? "Not available here yet"
      : running
        ? `Running${service.version ? ` · version ${service.version}` : ""}`
        : service.installed
          ? `Installed · ${service.state ?? "not running"}`
          : "Not installed";
  const d = RECORDING_DEFAULTS;
  const minutes = (v: number) => `${v} min`;

  return (
    <>
      {error ? <div className="settings-note bad">{error}</div> : null}
      {service?.versionMismatch ? (
        <div className="settings-note bad">
          The service is version {service.version}, not this app’s. Install it again to run the
          matching one.
        </div>
      ) : null}

      <SettingsGroup heading="Service" hint="on this computer">
        <SettingRow
          label={stateWord}
          description={
            service && !service.supported
              ? service.unsupportedReason
              : service?.supported && !service.installed && !service.binary
                ? "zapper-server was not found beside zapper or on the PATH, so it cannot be installed from here yet."
                : `Records what you schedule from the guide, even with zapper closed. It runs as you (a systemd user service), listens on ${service?.address ?? "127.0.0.1"} (and on your network only if you allow it below), and keeps the catalogue current like the app does. Removing it keeps your recordings.`
          }
        >
          {service?.supported ? (
            service.installed ? (
              <button
                className="ctl"
                disabled={busy}
                onClick={() => void act(backend.removeRecordingService)}
              >
                Remove
              </button>
            ) : (
              <button
                className="ctl"
                disabled={busy || !service.binary}
                onClick={() => void act(() => backend.installRecordingService(linger))}
              >
                Install
              </button>
            )
          ) : null}
        </SettingRow>
        {service?.supported ? (
          <SettingRow
            label="Keep running when logged out"
            htmlFor="rec-linger"
            description="Lets it record while nobody is logged in to this computer."
          >
            <Switch
              id="rec-linger"
              checked={linger}
              disabled={busy}
              onChange={(on) =>
                service.installed ? void act(() => backend.setRecordingLinger(on)) : setLinger(on)
              }
            />
          </SettingRow>
        ) : null}
      </SettingsGroup>

      {!running || !settings ? (
        <SettingsGroup heading="Recordings">
          <SettingRow
            label="Not available"
            description="These settings are the service’s own; start it to change them."
          />
        </SettingsGroup>
      ) : (
        <>
          <SettingsGroup heading="Recordings">
            <SettingRow
              label="Folder"
              htmlFor="rec-folder"
              layout="form"
              description={
                <>
                  Where recordings go.
                  {status.diskFree != null ? ` ${bytes(status.diskFree)} free on that disk.` : ""}
                </>
              }
              modified={settings.outputDir !== settings.defaultOutputDir}
              onReset={() => {
                setFolderDraft(null);
                patch({ outputDir: settings.defaultOutputDir });
              }}
              resetTo={settings.defaultOutputDir}
            >
              <input
                id="rec-folder"
                className="field"
                value={folderDraft ?? settings.outputDir}
                spellCheck={false}
                autoComplete="off"
                onChange={(e) => setFolderDraft(e.target.value)}
                // Sent when the field is left or Enter is pressed (through
                // patch, a moment later like every change here). From then
                // the field shows the settings again, which hold the typed
                // folder until the service answers with what it saved.
                onBlur={() => commitFolder()}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitFolder();
                }}
              />
            </SettingRow>
            <SettingRow
              label="File names"
              layout="stack"
              description="How each recording is named, and the folders it goes in. A recording that switched feeds is numbered part1, part2; a name already taken gets (2) rather than replace a file."
              modified={(settings.naming ?? d.naming) !== d.naming}
              onReset={() => patch({ naming: d.naming })}
              resetTo={presetLabel(d.naming)}
            >
              <NamingField settings={settings} patch={patch} />
            </SettingRow>
            <SettingRow
              label="Start early"
              htmlFor="rec-before"
              description="Recorded before the guide’s start, for a programme that begins early."
              modified={settings.padBefore !== d.padBefore}
              onReset={() => patch({ padBefore: d.padBefore })}
              resetTo={minutes(padMinutes(d.padBefore))}
            >
              <RangeField
                id="rec-before"
                min={0}
                max={15}
                step={1}
                value={padMinutes(settings.padBefore)}
                mark={padMinutes(d.padBefore)}
                format={minutes}
                onChange={(v) => patch({ padBefore: padSeconds(v) })}
              />
            </SettingRow>
            <SettingRow
              label="Stop late"
              htmlFor="rec-after"
              description="Recorded after the guide’s end, for a programme that overruns."
              modified={settings.padAfter !== d.padAfter}
              onReset={() => patch({ padAfter: d.padAfter })}
              resetTo={minutes(padMinutes(d.padAfter))}
            >
              <RangeField
                id="rec-after"
                min={0}
                max={60}
                step={1}
                value={padMinutes(settings.padAfter)}
                mark={padMinutes(d.padAfter)}
                format={minutes}
                onChange={(v) => patch({ padAfter: padSeconds(v) })}
              />
            </SettingRow>
            <SettingRow
              label="Add a clock subtitle track"
              htmlFor="rec-clock"
              description="A subtitle track showing when each moment was live. Players leave it off until you choose it."
              modified={settings.clockTrack !== d.clockTrack}
              onReset={() => patch({ clockTrack: d.clockTrack })}
              resetTo={d.clockTrack ? "on" : "off"}
            >
              <Switch
                id="rec-clock"
                checked={settings.clockTrack}
                onChange={(clockTrack) => patch({ clockTrack })}
              />
            </SettingRow>
          </SettingsGroup>
          <NetworkAccess />
        </>
      )}

      {/* The app's own setting, not the service's: the same one Settings ›
          Account shows, which the player's reconnects and the service's
          captures both count against. Here because a recording is where a
          second stream is most often wanted. */}
      <SettingsGroup heading="Connections">
        <SettingRow
          label="Simultaneous streams"
          htmlFor="rec-streams"
          description="For a playlist, which cannot say how many streams it allows at once (Xtream accounts and HDHomeRun tuners report their own). A recording waits for a free connection rather than take one someone is watching on. The same setting as in Account."
          modified={streams !== dStreams}
          onReset={() => s.patch({ simultaneousStreams: dStreams })}
          resetTo={String(dStreams)}
        >
          <RangeField
            id="rec-streams"
            min={STREAMS_MIN}
            max={STREAMS_MAX}
            step={1}
            value={streams}
            mark={dStreams}
            format={(v) => String(v)}
            onChange={(simultaneousStreams) => s.patch({ simultaneousStreams })}
          />
        </SettingRow>
      </SettingsGroup>
    </>
  );
}
