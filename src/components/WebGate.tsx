// In a browser served by the recording service: the sign-in before the app.
//
// Asks the service who this browser is (`/api/session`) and shows either the
// app or the sign-in. Signing in sets a cookie the page cannot read, and from
// then on every request carries it; an answer of 401 anywhere (the device
// revoked in the app's Settings, or signed out in another tab) brings the
// sign-in back. The app is mounted fresh after each sign-in, so nothing from
// the last session's store is left behind.

import { Fragment, useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { errorMessage } from "../logic/async";
import { onSignedOut, session, signIn } from "../web";
import { Bolt } from "./icons";

type Gate =
  | { state: "checking" }
  | { state: "out"; note: string | null }
  | { state: "in"; key: number }
  | { state: "unreachable"; message: string };

export function WebGate({ children }: { children: ReactNode }) {
  const [gate, setGate] = useState<Gate>({ state: "checking" });
  const signedIn = useRef(0);

  const check = () => {
    setGate({ state: "checking" });
    session()
      .then((device) =>
        setGate(device ? { state: "in", key: ++signedIn.current } : { state: "out", note: null }),
      )
      .catch((e: unknown) => setGate({ state: "unreachable", message: errorMessage(e) }));
  };

  useEffect(check, []);

  useEffect(
    () =>
      onSignedOut(() =>
        setGate((g) =>
          g.state === "in" ? { state: "out", note: "You were signed out." } : g,
        ),
      ),
    [],
  );

  switch (gate.state) {
    case "in":
      return <Fragment key={gate.key}>{children}</Fragment>;
    case "out":
      return (
        <SignIn
          note={gate.note}
          onSignedIn={() => setGate({ state: "in", key: ++signedIn.current })}
        />
      );
    case "unreachable":
      return (
        <Card>
          <p className="signin-error" role="alert">
            {gate.message}
          </p>
          <button className="ctl signin-submit" onClick={check}>
            Try again
          </button>
        </Card>
      );
    case "checking":
      return <Card />;
  }
}

function Card({ children }: { children?: ReactNode }) {
  return (
    <main className="signin">
      <div className="signin-card">
        <div className="signin-title">
          <Bolt className="signin-bolt" />
          zapper
        </div>
        {children}
      </div>
    </main>
  );
}

function SignIn({ note, onSignedIn }: { note: string | null; onSignedIn: () => void }) {
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (busy || password === "") return;
    setBusy(true);
    setError(null);
    signIn(password)
      .then(() => {
        setPassword("");
        onSignedIn();
      })
      .catch((err: unknown) => {
        setError(errorMessage(err));
        setBusy(false);
      });
  };

  return (
    <Card>
      <form className="signin-form" onSubmit={submit}>
        <p className="signin-lead">
          Sign in to the recording service with the password set in the zapper app’s Settings ›
          Recording.
        </p>
        {note ? <p className="signin-note">{note}</p> : null}
        <label className="signin-label" htmlFor="signin-password">
          Password
        </label>
        <input
          id="signin-password"
          className="field"
          type="password"
          autoComplete="current-password"
          autoFocus
          value={password}
          disabled={busy}
          onChange={(e) => setPassword(e.target.value)}
        />
        {error ? (
          <p className="signin-error" role="alert">
            {error}
          </p>
        ) : null}
        <button className="ctl signin-submit" type="submit" disabled={busy || password === ""}>
          {busy ? "Signing in…" : "Sign in"}
        </button>
        <p className="signin-foot">
          This browser is then listed under Other devices in the app, where it can be signed out.
        </p>
      </form>
    </Card>
  );
}
