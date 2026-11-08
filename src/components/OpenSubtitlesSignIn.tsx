// "Sign in to OpenSubtitles": the form the subtitle finder asks for when a
// first download is chosen, and Settings › Online services offers too. The
// password is held only while it is typed and is cleared after any answer;
// it is sent to the backend and never kept here.

import { useRef, useState, type FormEvent } from "react";
import { backend, capabilities, passwordsInClear } from "../backend";
import { errorMessage } from "../logic/async";
import type { OnlineServicesDto } from "../types";

const SIGN_UP = "https://www.opensubtitles.com/en/users/sign_up";

export function OpenSubtitlesSignIn({
  onSignedIn,
  compact,
}: {
  onSignedIn: (s: OnlineServicesDto) => void;
  compact?: boolean;
}) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const sending = useRef(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (sending.current || username.trim() === "" || password === "") return;
    sending.current = true;
    setBusy(true);
    setError(null);
    try {
      const next = await backend.signInOpenSubtitles(username.trim(), password);
      setPassword("");
      onSignedIn(next);
    } catch (err) {
      setPassword("");
      setError(errorMessage(err));
    } finally {
      sending.current = false;
      setBusy(false);
    }
  };

  return (
    <form
      className={`os-signin${compact ? " compact" : ""}`}
      onSubmit={(e) => void submit(e)}
    >
      <input
        className="field"
        type="text"
        name="username"
        autoComplete="username"
        aria-label="OpenSubtitles username"
        placeholder="Username"
        spellCheck={false}
        value={username}
        disabled={busy}
        onChange={(e) => setUsername(e.target.value)}
      />
      <input
        className="field"
        type="password"
        name="password"
        autoComplete="current-password"
        aria-label="OpenSubtitles password"
        placeholder="Password"
        value={password}
        disabled={busy}
        onChange={(e) => setPassword(e.target.value)}
      />
      <button
        type="submit"
        className="ctl"
        disabled={busy || username.trim() === "" || password === ""}
      >
        Sign in
      </button>
      {passwordsInClear ? (
        <div className="settings-note bad os-signin-why" role="note">
          This page uses plain HTTP, so a password typed here crosses the network unencrypted. To
          encrypt it, turn off “Plain HTTP on the network” in the zapper app’s Settings →
          Recording.
        </div>
      ) : null}
      {error ? (
        <div className="net-why os-signin-why" role="alert">
          {error}
        </div>
      ) : null}
      <p className="os-signin-note">
        Downloads count against your own free OpenSubtitles account.{" "}
        <a
          href={SIGN_UP}
          target="_blank"
          rel="noreferrer noopener"
          onClick={(e) => {
            // The app's window has nowhere to open a link: its backend does.
            if (!capabilities.window) return;
            e.preventDefault();
            void backend.openOpenSubtitlesSignUp().catch(() => {});
          }}
        >
          Create an account
        </a>
      </p>
    </form>
  );
}
