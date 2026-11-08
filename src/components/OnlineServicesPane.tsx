// Settings › Online services: one group per service, each with its switch and
// a line on what it sends. Off means no request to that service at all.

import { useState, type ReactNode } from "react";
import { backend } from "../backend";
import { errorMessage } from "../logic/async";
import { dateTime, hhmm } from "../logic/format";
import { OPENSUBTITLES_LANGUAGES, searchLanguages } from "../logic/onlineSubtitles";
import { languageName } from "../logic/tracks";
import { useOnlineServices } from "../onlineSubtitles";
import type { OpenSubtitlesStatusDto } from "../types";
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

function downloadsLine(os: OpenSubtitlesStatusDto): string {
  const who = `Signed in as ${os.signedInAs}`;
  if (os.downloadsLeft === null) return who;
  if (os.downloadsLeft > 0) return `${who} · ${os.downloadsLeft} downloads left today`;
  if (os.downloadsResetAt === null) return `${who} · no downloads left today`;
  const sameDay = new Date(os.downloadsResetAt * 1000).toDateString() === new Date().toDateString();
  return `${who} · no downloads left until ${
    sameDay ? hhmm(os.downloadsResetAt) : dateTime(os.downloadsResetAt)
  }`;
}

export function OnlineServicesPane({ standing = null }: { standing?: string | null }) {
  const { status, reload, set } = useOnlineServices();
  const [error, setError] = useState<string | null>(null);
  const [form, setForm] = useState(false);

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
  // A sign-in OpenSubtitles refused is not a sign-in, whoever it names.
  const signedIn = os.signedInAs !== null && !os.signInRefused;
  const first = os.languages[0] ?? "";
  const second = os.languages[1] ?? "";
  const pick = (a: string, b: string) => {
    const langs = [a, b].filter((x, i, all) => x !== "" && all.indexOf(x) === i);
    void save({ languages: langs });
  };
  const options = (skip: string) =>
    SORTED_LANGUAGES.filter(([code]) => code !== skip).map(([code, name]) => (
      <option key={code} value={code}>
        {name}
      </option>
    ));

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
        description="Searched for, at most two."
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
            {options(second)}
          </select>
          <select
            id="online-os-lang-2"
            aria-label="Second language"
            value={second}
            disabled={!os.enabled || first === ""}
            onChange={(e) => pick(first, e.target.value)}
          >
            <option value="">None</option>
            {options(first)}
          </select>
        </div>
      </SettingRow>
      <SettingRow
        label="Sign-in"
        layout={signedIn ? "side" : os.signInRefused || form ? "stack" : "side"}
        description={
          signedIn
            ? downloadsLine(os)
            : os.signInRefused
              ? "OpenSubtitles did not accept the saved sign-in."
              : "Searching needs no sign-in; downloading does, with your own free account."
        }
      >
        {signedIn ? (
          <button className="ctl" onClick={() => void run(() => backend.signOutOpenSubtitles())}>
            Sign out
          </button>
        ) : os.signInRefused || form ? (
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
