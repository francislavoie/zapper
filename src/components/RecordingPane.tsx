// Settings › Recording: the recording service on this computer (install,
// remove, whether it keeps running after logging out) and what it does with
// every recording (where, how much padding, the clock track).
//
// The settings are the service's own, read and written through its API, so
// they can only be changed while it is running. Like the rest of Settings, a
// change shows at once and is saved a moment later, and each one that
// differs from the service's default has a ↺. Installing the service is the
// person's decision: nothing here runs until a button is pressed.

import { useCallback, useEffect, useRef, useState } from "react";
import { refreshRecordings } from "../actions";
import { backend } from "../backend";
import { errorMessage } from "../logic/async";
import { RECORDING_DEFAULTS, bytes, padMinutes, padSeconds } from "../logic/recordings";
import { useDispatch } from "../store";
import type { RecordingSettingsDto, RecordingStatusDto, ServiceStatusDto } from "../types";
import { RangeField, SettingRow, SettingsGroup } from "./SettingsParts";
import { Switch } from "./Switch";

const SAVE_AFTER_MS = 400;

export function RecordingPane() {
  const dispatch = useDispatch();
  const [service, setService] = useState<ServiceStatusDto | null>(null);
  const [status, setStatus] = useState<RecordingStatusDto | null>(null);
  const [settings, setSettings] = useState<RecordingSettingsDto | null>(null);
  const [folder, setFolder] = useState("");
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
      if (!pending.current) {
        setSettings(set);
        setFolder(set.outputDir);
      }
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
        setFolder(saved.outputDir);
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
                : `Records what you schedule from the guide, even with zapper closed. It runs as you (a systemd user service), listens on ${service?.address ?? "127.0.0.1"} only, and keeps the catalogue current like the app does. Removing it keeps your recordings.`
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
                  Each show gets a folder inside, and each airing a file named for it and its start,
                  the way Jellyfin and Plex expect.
                  {status.diskFree != null ? ` ${bytes(status.diskFree)} free on that disk.` : ""}
                </>
              }
              modified={settings.outputDir !== settings.defaultOutputDir}
              onReset={() => patch({ outputDir: "" })}
              resetTo={settings.defaultOutputDir}
            >
              <input
                id="rec-folder"
                className="field"
                value={folder}
                spellCheck={false}
                autoComplete="off"
                onChange={(e) => setFolder(e.target.value)}
                onBlur={() => folder !== settings.outputDir && patch({ outputDir: folder })}
                onKeyDown={(e) => {
                  if (e.key === "Enter") patch({ outputDir: folder });
                }}
              />
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

          <SettingsGroup heading="Connections">
            <SettingRow
              label="Streams a playlist account allows at once"
              htmlFor="rec-streams"
              description="Xtream accounts report their own limit; playlists cannot. A recording waits for a free connection rather than take one someone is watching on."
              modified={settings.simultaneousStreams !== d.simultaneousStreams}
              onReset={() => patch({ simultaneousStreams: d.simultaneousStreams })}
              resetTo={String(d.simultaneousStreams)}
            >
              <RangeField
                id="rec-streams"
                min={1}
                max={6}
                step={1}
                value={settings.simultaneousStreams}
                mark={d.simultaneousStreams}
                format={(v) => String(v)}
                onChange={(simultaneousStreams) => patch({ simultaneousStreams })}
              />
            </SettingRow>
          </SettingsGroup>
        </>
      )}
    </>
  );
}
