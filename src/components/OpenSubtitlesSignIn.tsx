// "Sign in to OpenSubtitles": the form the subtitle finder asks for when a
// first download is chosen, and Settings › Online services offers too. The
// password is held only while it is typed and is cleared after any answer;
// it is sent to the backend and never kept here.

import { useRef, useState, type FormEvent } from "react";
import { api } from "../api";
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
      const next = await api.signInOpenSubtitles(username.trim(), password);
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
      {error ? (
        <div className="net-why os-signin-why" role="alert">
          {error}
        </div>
      ) : null}
      <p className="os-signin-note">
        Downloads count against your own free OpenSubtitles account.{" "}
        <a href={SIGN_UP} target="_blank" rel="noreferrer noopener">
          Create an account
        </a>
      </p>
    </form>
  );
}
