// Settings › Recording › Network access: whether other devices on the
// network may reach the recording service, on which port, over HTTPS or
// plain HTTP, the password they sign in with, and the devices signed in.
//
// All of it is the service's (docs/recording.md, "Network access"), changed
// through its API from this app alone. Each change is sent as it is made
// rather than a moment later like the rest of Settings: turning access on or
// changing the port moves where the service listens, and the answer (a port
// another program holds) belongs beside the control that caused it.

import { useCallback, useEffect, useRef, useState } from "react";
import { backend } from "../backend";
import { errorMessage } from "../logic/async";
import {
  DEFAULT_PORT,
  canEnable,
  deviceLine,
  networkHint,
  parsePort,
  passwordProblem,
  portProblem,
} from "../logic/network";
import type { NetworkDto, NetworkSettingsDto } from "../types";
import { SettingRow, SettingsGroup } from "./SettingsParts";
import { Switch } from "./Switch";

const POLL_MS = 5_000;
const COPIED_MS = 1_500;

/** The service's words start lower-case, to follow "…:" in a log line;
 *  here they stand alone. */
const sentence = (s: string) => {
  const t = s.charAt(0).toUpperCase() + s.slice(1);
  return /[.!?]$/.test(t) ? t : `${t}.`;
};

export function NetworkAccess() {
  const [net, setNet] = useState<NetworkDto | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** The port as typed, while it is being edited. */
  const [portDraft, setPortDraft] = useState<string | null>(null);
  const [password, setPassword] = useState("");
  const [again, setAgain] = useState("");
  const [passwordNote, setPasswordNote] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const portSending = useRef(false);

  const load = useCallback(async () => {
    try {
      setNet(await backend.recordingNetwork());
    } catch (e) {
      setNet(null);
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void load();
    // Devices sign in and are used while the page is open.
    const id = setInterval(() => void load(), POLL_MS);
    return () => clearInterval(id);
  }, [load]);

  useEffect(() => {
    if (!copied) return;
    const id = setTimeout(() => setCopied(null), COPIED_MS);
    return () => clearTimeout(id);
  }, [copied]);

  const run = async (what: () => Promise<NetworkDto>) => {
    setBusy(true);
    setError(null);
    try {
      setNet(await what());
      return true;
    } catch (e) {
      setError(sentence(errorMessage(e)));
      void load();
      return false;
    } finally {
      setBusy(false);
    }
  };

  const apply = (p: Partial<NetworkSettingsDto>) => {
    if (!net) return Promise.resolve(false);
    const next: NetworkSettingsDto = {
      enabled: net.enabled,
      port: net.port,
      https: net.https,
      ...p,
    };
    return run(() => backend.setRecordingNetwork(next));
  };

  const commitPort = async () => {
    // Enter sends it, and the field then loses its focus, which would send
    // it again: a second move of the listeners while the first is under way.
    if (portDraft === null || !net || portSending.current) return;
    const port = parsePort(portDraft);
    if (port === null) return;
    if (port === net.port) {
      setPortDraft(null);
      return;
    }
    portSending.current = true;
    try {
      if (await apply({ port })) setPortDraft(null);
    } finally {
      portSending.current = false;
    }
  };

  const setNewPassword = async () => {
    if (passwordProblem(password, again)) return;
    const hadDevices = (net?.devices.length ?? 0) > 0;
    if (await run(() => backend.setRecordingPassword(password))) {
      setPassword("");
      setAgain("");
      setPasswordNote(hadDevices ? "Password set. Every device was signed out." : "Password set.");
    }
  };

  const copy = (text: string) => {
    void navigator.clipboard?.writeText(text).then(
      () => setCopied(text),
      () => setCopied(null),
    );
  };

  if (!net) {
    return error ? <div className="settings-note bad">{error}</div> : null;
  }

  const portWhy = portDraft !== null ? portProblem(portDraft) : null;
  // Said once there is something to say: not "at least 8 characters" for
  // an empty field, nor "differ" before the second one is typed.
  const passwordWhy =
    password === "" ? null : (passwordProblem(password, again === "" ? password : again) ?? null);
  const passwordReady = password !== "" && again !== "" && !passwordProblem(password, again);

  return (
    <>
      {error ? (
        <div className="settings-note bad" role="alert">
          {error}
        </div>
      ) : null}
      {net.error ? <div className="settings-note bad">{sentence(net.error)}</div> : null}

      <SettingsGroup heading="Network access" hint={networkHint(net)}>
        <SettingRow
          label="Allow other devices on your network"
          htmlFor="rec-net"
          description={
            canEnable(net)
              ? "Lets a browser on another computer, phone or TV reach the recording service at the addresses below, once signed in with the password. Off, it listens on this computer only."
              : "Set a sign-in password below first: other devices need one to get in."
          }
          modified={net.enabled}
          onReset={() => void apply({ enabled: false })}
          resetTo="off"
        >
          <Switch
            id="rec-net"
            checked={net.enabled}
            disabled={busy || !canEnable(net)}
            onChange={(enabled) => void apply({ enabled })}
          />
        </SettingRow>
        <SettingRow
          label="Port"
          htmlFor="rec-port"
          description={
            portWhy ? (
              <span className="set-warn" role="alert">
                {portWhy}
              </span>
            ) : (
              "Where this app and other devices reach the service."
            )
          }
          modified={net.port !== DEFAULT_PORT}
          onReset={() => {
            setPortDraft(null);
            void apply({ port: DEFAULT_PORT });
          }}
          resetTo={String(DEFAULT_PORT)}
        >
          <input
            id="rec-port"
            className="field net-port"
            inputMode="numeric"
            value={portDraft ?? String(net.port)}
            disabled={busy}
            spellCheck={false}
            autoComplete="off"
            aria-invalid={portWhy ? true : undefined}
            onChange={(e) => setPortDraft(e.target.value)}
            onBlur={() => void commitPort()}
            onKeyDown={(e) => {
              if (e.key === "Enter") void commitPort();
              if (e.key === "Escape" && portDraft !== null) {
                e.stopPropagation();
                setPortDraft(null);
              }
            }}
          />
        </SettingRow>
        <SettingRow
          label="Plain HTTP on the network"
          htmlFor="rec-plain"
          description={
            net.https ? (
              "Off: other devices use HTTPS, with the service’s own certificate. This app uses plain HTTP either way, since it never leaves this computer."
            ) : (
              <span className="set-warn">
                Everything other devices send and receive, the password included, crosses the
                network unencrypted, for anyone on it to read. Only on a network you trust.
              </span>
            )
          }
          modified={!net.https}
          onReset={() => void apply({ https: true })}
          resetTo="off"
        >
          <Switch
            id="rec-plain"
            checked={!net.https}
            disabled={busy}
            onChange={(plain) => void apply({ https: !plain })}
          />
        </SettingRow>
        {net.https && net.fingerprint ? (
          <SettingRow
            label="Certificate"
            layout="stack"
            description="A browser warns about it the first time, since the service made it rather than an authority. Before going on, check that the SHA-256 fingerprint in the browser’s certificate details is this one."
          >
            <div className="net-copyable">
              <code className="net-fingerprint" data-selectable="">
                {/* Wrapped only between pairs, which are what is compared. */}
                {net.fingerprint.split(":").map((pair, i) => (
                  <span key={i}>
                    {i ? ":" : ""}
                    {i ? <wbr /> : null}
                    {pair}
                  </span>
                ))}
              </code>
              <button type="button" className="ctl" onClick={() => copy(net.fingerprint ?? "")}>
                {copied === net.fingerprint ? "Copied" : "Copy"}
              </button>
            </div>
          </SettingRow>
        ) : null}
        {net.enabled ? (
          <SettingRow
            label="Addresses"
            layout="stack"
            description={
              net.addresses.length
                ? "Open one of these on the other device."
                : "This computer has no network address right now."
            }
          >
            {net.addresses.length ? (
              <ul className="net-addresses">
                {net.addresses.map((a) => (
                  <li key={a} className="net-copyable">
                    <code data-tip-overflow="" data-selectable="">
                      {a}
                    </code>
                    <button type="button" className="ctl" onClick={() => copy(a)}>
                      {copied === a ? "Copied" : "Copy"}
                    </button>
                  </li>
                ))}
              </ul>
            ) : null}
          </SettingRow>
        ) : null}
      </SettingsGroup>

      <SettingsGroup heading="Other devices" hint="sign-in">
        <SettingRow
          label={net.passwordSet ? "Change the password" : "Sign-in password"}
          layout="stack"
          description={
            passwordNote && password === ""
              ? passwordNote
              : net.passwordSet
                ? "A new password signs every device out. The service keeps only a hash of it."
                : "What other devices sign in with: at least 8 characters. The service keeps only a hash of it."
          }
        >
          <form
            className="net-password"
            onSubmit={(e) => {
              e.preventDefault();
              void setNewPassword();
            }}
          >
            <input
              className="field"
              type="password"
              autoComplete="new-password"
              aria-label="New password"
              placeholder="New password"
              value={password}
              disabled={busy}
              onChange={(e) => {
                setPassword(e.target.value);
                setPasswordNote(null);
              }}
            />
            <input
              className="field"
              type="password"
              autoComplete="new-password"
              aria-label="The new password again"
              placeholder="Again"
              value={again}
              disabled={busy}
              onChange={(e) => setAgain(e.target.value)}
            />
            <button type="submit" className="ctl" disabled={busy || !passwordReady}>
              {net.passwordSet ? "Change" : "Set"}
            </button>
          </form>
          {passwordWhy ? (
            <div className="net-why" role="alert">
              {passwordWhy}
            </div>
          ) : null}
        </SettingRow>
        {net.devices.length === 0 ? (
          <SettingRow
            label="No device is signed in"
            description="A device signed in is listed here, with when it was last used, until it is revoked."
          />
        ) : (
          net.devices.map((d) => (
            <SettingRow
              key={d.id}
              label={<span data-tip={d.userAgent || undefined}>{d.name}</span>}
              description={deviceLine(d, Math.floor(Date.now() / 1000))}
            >
              <button
                type="button"
                className="ctl"
                disabled={busy}
                onClick={() => void run(() => backend.revokeRecordingDevice(d.id))}
              >
                Revoke
              </button>
            </SettingRow>
          ))
        )}
      </SettingsGroup>
    </>
  );
}
