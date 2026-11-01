// The account's connections, under its details in Settings → Account: how
// many streams it allows, how many are open on every device, and when it
// expires — as the provider counts them where it can (an Xtream server, an
// HDHomeRun's tuners), or as set here where it cannot (a playlist, a
// Tvheadend server).
//
// This is what decides whether zapper may reconnect by itself after the
// network fails: only with a connection to spare, so that two copies of
// zapper on one account never take a single connection back and forth.

import { useEffect, useState } from "react";
import { backend } from "../backend";
import type { SettingsControl } from "../hooks/useSettings";
import { connectionsLine } from "../logic/connections";
import { DEFAULT_SETTINGS, STREAMS_MAX, STREAMS_MIN } from "../logic/settings";
import type { ConnectionsDto } from "../types";
import { RangeField, SettingRow, SettingsGroup } from "./SettingsParts";

export function ConnectionsPane({ s }: { s: SettingsControl }) {
  const [status, setStatus] = useState<ConnectionsDto | null>(null);
  const [failed, setFailed] = useState(false);
  const streams = s.settings.simultaneousStreams;
  const d = DEFAULT_SETTINGS.simultaneousStreams;

  // Asked when the pane opens, and again when the setting it reads moves.
  // The backend keeps the provider's answer for a few seconds, so opening
  // and closing Settings does not ask the server each time.
  useEffect(() => {
    let live = true;
    backend
      .connectionStatus()
      .then((c) => {
        if (!live) return;
        setStatus(c);
        setFailed(false);
      })
      .catch(() => {
        if (live) setFailed(true);
      });
    return () => {
      live = false;
    };
  }, [streams]);

  const line = status ? connectionsLine(status) : failed ? "Couldn’t ask the account." : "Asking…";

  return (
    <SettingsGroup heading="Connections">
      <SettingRow
        label="In use"
        description="Zapper reconnects by itself after the network fails only when the account has a connection to spare, so it never takes one another device is watching on."
      >
        <span className="set-value" data-tip-overflow={line}>
          {line}
        </span>
      </SettingRow>
      {status?.declared ? (
        <SettingRow
          label="Simultaneous streams"
          htmlFor="set-streams"
          description="How many streams your provider lets this account play at once. A playlist can’t say, so zapper asks you. With one, zapper only reconnects by itself when the server couldn’t be reached at all, and a recording waits for a free connection."
          modified={streams !== d}
          onReset={() => s.patch({ simultaneousStreams: d })}
          resetTo={String(d)}
        >
          <RangeField
            id="set-streams"
            min={STREAMS_MIN}
            max={STREAMS_MAX}
            step={1}
            value={streams}
            mark={d}
            format={(v) => String(v)}
            onChange={(simultaneousStreams) => s.patch({ simultaneousStreams })}
          />
        </SettingRow>
      ) : null}
    </SettingsGroup>
  );
}
