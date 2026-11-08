// Settings › Online services: one group per service, each with its switch and
// a line on what it sends. Off means no request to that service at all.

import { useEffect, useState, type ReactNode } from "react";
import { backend } from "../backend";
import { errorMessage } from "../logic/async";
import {
  OPENSUBTITLES_LANGUAGES,
  isSignedIn,
  searchLanguages,
  signInLine,
} from "../logic/onlineSubtitles";
import { languageName } from "../logic/tracks";
import { useOnlineServices } from "../onlineSubtitles";
import { OpenSubtitlesSignIn } from "./OpenSubtitlesSignIn";
import { SettingRow, SettingsGroup } from "./SettingsParts";
import { Switch } from "./Switch";

const OPENSUBTITLES_SENDS =
  "Finds subtitles for the movie or episode playing. Sends its TMDB id, or its name and year, with the season and episode and your subtitle languages; downloads send your OpenSubtitles sign-in. Never your TV provider's account.";

/** One service: its switch and what it sends, then whatever else it has. */
export function ServiceGroup({
  name,
  sends,
  on,
  onToggle,
  children,
}: {
  name: string;
  sends: string;
  on: boolean;
  onToggle: (next: boolean) => void;
  children?: ReactNode;
}) {
  const id = `online-${name.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`;
  return (
    <SettingsGroup heading={name}>
      <SettingRow label={name} htmlFor={id} description={sends}>
        <Switch id={id} checked={on} onChange={onToggle} />
      </SettingRow>
      {children}
    </SettingsGroup>
  );
}

const SORTED_LANGUAGES = OPENSUBTITLES_LANGUAGES.map(
  (l) => [l.code, languageName(l.code) ?? l.code] as const,
).sort((a, b) => a[1].localeCompare(b[1]));

const KNOWN_LANGUAGES = new Set(OPENSUBTITLES_LANGUAGES.map((l) => l.code));

export function OnlineServicesPane({ standing = null }: { standing?: string | null }) {
  const { status, reload, set } = useOnlineServices();
  const [error, setError] = useState<string | null>(null);
  const [form, setForm] = useState(false);
  // Read afresh each time the pane opens: the account's downloads left
  // change with every download, here or on another device.
  useEffect(() => reload(), [reload]);

  if (status.status !== "ready") {
    return status.status === "error" ? (
      <div className="settings-note bad" role="alert">
        Couldn't read the online services: {status.message}{" "}
        <button className="ctl" onClick={reload}>
          Try again
        </button>
      </div>
    ) : (
      <p className="settings-note">Loading…</p>
    );
  }
  const os = status.data.openSubtitles;

  const run = async (call: () => Promise<Parameters<typeof set>[0]>) => {
    setError(null);
    try {
      set(await call());
    } catch (e) {
      setError(errorMessage(e));
    }
  };
  const save = (patch: { enabled?: boolean; languages?: string[] }) =>
    run(() => backend.setOpenSubtitles(patch));

  const locales = typeof navigator === "undefined" ? [] : navigator.languages;
  const auto = searchLanguages([], standing, locales)
    .map((l) => languageName(l) ?? l)
    .join(", ");
  const signedIn = isSignedIn(os);
  const first = os.languages[0] ?? "";
  const second = os.languages[1] ?? "";
  const pick = (a: string, b: string) => {
    const langs = [a, b].filter((x, i, all) => x !== "" && all.indexOf(x) === i);
    void save({ languages: langs });
  };
  // A saved code the list doesn't have is still shown, so the select never
  // says something other than what is saved.
  const options = (skip: string, own: string) => {
    const rows = SORTED_LANGUAGES.filter(([code]) => code !== skip);
    if (own !== "" && !KNOWN_LANGUAGES.has(own)) rows.push([own, languageName(own) ?? own]);
    return rows.map(([code, name]) => (
      <option key={code} value={code}>
        {name}
      </option>
    ));
  };

  return (
    <ServiceGroup
      name="OpenSubtitles"
      sends={OPENSUBTITLES_SENDS}
      on={os.enabled}
      onToggle={(enabled) => void save({ enabled })}
    >
      <SettingRow
        label="Languages"
        htmlFor="online-os-lang-1"
        description="Up to two languages."
      >
        <div className="online-langs">
          <select
            id="online-os-lang-1"
            aria-label="First language"
            value={first}
            disabled={!os.enabled}
            onChange={(e) => pick(e.target.value, e.target.value === "" ? "" : second)}
          >
            <option value="">Automatic ({auto})</option>
            {options(second, first)}
          </select>
          <select
            id="online-os-lang-2"
            aria-label="Second language"
            value={second}
            disabled={!os.enabled || first === ""}
            onChange={(e) => pick(first, e.target.value)}
          >
            <option value="">None</option>
            {options(first, second)}
          </select>
        </div>
      </SettingRow>
      <SettingRow
        label="Sign-in"
        layout={!signedIn && os.enabled && (os.signInRefused || form) ? "stack" : "side"}
        description={
          signedIn
            ? signInLine(os, Date.now() / 1000)
            : os.signInRefused
              ? "OpenSubtitles did not accept the saved sign-in."
              : "Searching needs no sign-in; downloading does, with your own free account."
        }
      >
        {signedIn ? (
          <button
            className="ctl"
            disabled={!os.enabled}
            onClick={() => void run(() => backend.signOutOpenSubtitles())}
          >
            Sign out
          </button>
        ) : os.enabled && (os.signInRefused || form) ? (
          <OpenSubtitlesSignIn
            compact
            onSignedIn={(next) => {
              setForm(false);
              set(next);
            }}
          />
        ) : (
          <button className="ctl" disabled={!os.enabled} onClick={() => setForm(true)}>
            Sign in to OpenSubtitles
          </button>
        )}
      </SettingRow>
      {error ? (
        <div className="net-why" role="alert">
          {error}
        </div>
      ) : null}
    </ServiceGroup>
  );
}
