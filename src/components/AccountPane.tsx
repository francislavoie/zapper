// The panel account: where to reach it, and as whom.
//
// The password is write-only. It is never sent back from the backend, so
// the field starts empty even when one is on file, and leaving it empty
// means "keep the one you have". That way it cannot appear in a devtools
// inspector, a screenshot of one, or a serialised copy of this component's
// state — and the interface never has to hold it in order to re-save the
// host beside it.

import { useEffect, useState } from "react";
import { backend } from "../backend";
import { errorMessage } from "../logic/async";
import type { AccountDto } from "../types";

export function AccountPane() {
  const [account, setAccount] = useState<AccountDto | null>(null);
  const [host, setHost] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<{ kind: "ok" | "bad"; text: string } | null>(null);

  useEffect(() => {
    let live = true;
    void backend
      .account()
      .then((a) => {
        if (!live) return;
        setAccount(a);
        setHost(a.host);
        setUsername(a.username);
      })
      .catch((e) => live && setNote({ kind: "bad", text: errorMessage(e) }));
    return () => {
      live = false;
    };
  }, []);

  const save = async () => {
    setBusy(true);
    setNote(null);
    try {
      await backend.setAccount(host, username, password);
      // Cleared rather than kept: there is nothing to re-submit, and a
      // password sitting in a form is a password waiting to be seen.
      setPassword("");
      const fresh = await backend.account();
      setAccount(fresh);
      setNote({ kind: "ok", text: "Saved. Testing…" });
      const result = await backend.testAccount();
      setNote({ kind: "ok", text: result });
    } catch (e) {
      setNote({ kind: "bad", text: errorMessage(e) });
    } finally {
      setBusy(false);
    }
  };

  const test = async () => {
    setBusy(true);
    setNote(null);
    try {
      setNote({ kind: "ok", text: await backend.testAccount() });
    } catch (e) {
      setNote({ kind: "bad", text: errorMessage(e) });
    } finally {
      setBusy(false);
    }
  };

  const changed =
    account !== null &&
    (host.trim() !== account.host || username.trim() !== account.username || password !== "");

  return (
    <>
      {/* One form, so the three labels share a column and the boxes line up. */}
      <div className="account-form">
        <div className="row-field">
          {/* "Xtream URL", not "panel address": it is the address of an Xtream
              Codes panel, and that is the phrase printed on the email a
              subscription arrives in. Somebody pasting a value in should be
              able to recognise which value it is being asked for. */}
          <label htmlFor="acct-host">Xtream URL</label>
          <input
            id="acct-host"
            className="field"
            value={host}
            spellCheck={false}
            autoComplete="off"
            placeholder="http://panel.example.com:8080"
            onChange={(e) => setHost(e.target.value)}
          />
        </div>

        <div className="row-field">
          <label htmlFor="acct-user">Username</label>
          <input
            id="acct-user"
            className="field"
            value={username}
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => setUsername(e.target.value)}
          />
        </div>

        <div className="row-field">
          <label htmlFor="acct-pass">Password</label>
          <input
            id="acct-pass"
            className="field"
            type="password"
            value={password}
            autoComplete="off"
            placeholder={account?.hasPassword ? "unchanged" : "required"}
            onChange={(e) => setPassword(e.target.value)}
          />
        </div>
      </div>

      <div className="settings-actions">
        <button className="ctl primary" disabled={busy || !changed} onClick={() => void save()}>
          Save
        </button>
        <button className="ctl" disabled={busy} onClick={() => void test()}>
          Test connection
        </button>
      </div>

      {/* Boxed, because it is news: it is the answer to the button that was
          just pressed, and it goes away again. */}
      {note ? (
        <p className={`settings-note ${note.kind === "bad" ? "bad" : ""}`}>{note.text}</p>
      ) : null}

      {/* Unboxed, because these are not news. They are true of the fields
          above whether or not anybody has touched them, and two grey boxes
          stacked under a form read as two warnings. */}
      <div className="settings-foot">
        {account?.source === "none" ? (
          <p>No account yet. Fill these in and save, and the catalogue will sync.</p>
        ) : null}
        <p>
          Stored in <code data-selectable>{account?.path ?? "—"}</code>, readable only by
          you. The password is never sent back to this window.
        </p>
      </div>
    </>
  );
}
