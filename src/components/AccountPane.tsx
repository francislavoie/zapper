// The account: an Xtream panel, or an M3U playlist with an optional guide.
//
// The password is write-only. It is never sent back from the backend, so
// the field starts empty even when one is on file, and leaving it empty
// means "keep the one you have". That way it cannot appear in a devtools
// inspector, a screenshot of one, or a serialised copy of this component's
// state — and the interface never has to hold it in order to re-save the
// host beside it.
//
// A playlist address gets the same rule when it carries credentials, which
// a provider's usually does (`get.php?username=…&password=…`). The backend
// sends it with those parts redacted and a flag saying so; the field then
// starts empty with the redacted form as its placeholder, and empty means
// keep. An address with nothing secret in it is shown as typed.
//
// The two kinds are not equivalent, and the pane says how before anyone
// picks one: the comparison under the switch is `SOURCE_COMPARISON`, kept as
// data in logic/sources.ts so it is checked against what the backend does.

import { useEffect, useState } from "react";
import { backend } from "../backend";
import { errorMessage } from "../logic/async";
import {
  SOURCE_COMPARISON,
  SOURCE_KINDS,
  asKind,
  kindInfo,
  m3uChanged,
  m3uFormFrom,
  planM3uSave,
  type M3uForm,
  type SourceKind,
} from "../logic/sources";
import type { AccountDto } from "../types";

const EMPTY_M3U: M3uForm = { playlist: "", epgText: "", userAgent: "", epgTouched: false };

export function AccountPane() {
  const [account, setAccount] = useState<AccountDto | null>(null);
  const [kind, setKind] = useState<SourceKind>("xtream");
  const [host, setHost] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [m3u, setM3u] = useState<M3uForm>(EMPTY_M3U);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<{ kind: "ok" | "bad"; text: string } | null>(null);

  const adopt = (a: AccountDto) => {
    setAccount(a);
    setHost(a.host);
    setUsername(a.username);
    setM3u(m3uFormFrom(a));
  };

  useEffect(() => {
    let live = true;
    void backend
      .account()
      .then((a) => {
        if (!live) return;
        adopt(a);
        setKind(asKind(a.kind));
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
      if (kind === "m3u") {
        if (!account) return;
        const plan = planM3uSave(m3u, account);
        if ("error" in plan) throw plan.error;
        await backend.setM3uAccount(plan.playlist, plan.epgUrls, plan.userAgent);
      } else {
        await backend.setAccount(host, username, password);
        // Cleared rather than kept: there is nothing to re-submit, and a
        // password sitting in a form is a password waiting to be seen.
        setPassword("");
      }
      const fresh = await backend.account();
      adopt(fresh);
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

  const savedKind = account ? asKind(account.kind) : null;
  // Switching kinds is itself a change: saving is what makes the other kind
  // the one in use.
  const switching = account !== null && kind !== savedKind;
  const changed =
    account !== null &&
    (switching ||
      (kind === "xtream"
        ? host.trim() !== account.host || username.trim() !== account.username || password !== ""
        : m3uChanged(m3u, account)));

  return (
    <>
      {/* The choice first, with what it means right under it: the two kinds
          do different amounts for you, and finding that out after setting
          one up is finding it out too late. */}
      <div className="row-field wrap">
        <label>Account type</label>
        <div className="choices" role="radiogroup" aria-label="Account type">
          {SOURCE_KINDS.map((k) => (
            <button
              key={k.id}
              role="radio"
              aria-checked={kind === k.id}
              className={`ctl ${kind === k.id ? "on" : ""}`}
              onClick={() => {
                setKind(k.id);
                setNote(null);
              }}
            >
              {k.label}
            </button>
          ))}
        </div>
      </div>
      <p className="source-blurb">
        {kindInfo(kind).blurb}
        {switching ? " Saving switches the account to this type." : ""}
      </p>

      <table className="source-compare">
        <thead>
          <tr>
            <th scope="col" />
            {SOURCE_KINDS.map((k) => (
              <th key={k.id} scope="col" className={kind === k.id ? "sel" : ""}>
                {k.label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {SOURCE_COMPARISON.map((row) => (
            <tr key={row.aspect}>
              <th scope="row">{row.aspect}</th>
              <td className={kind === "xtream" ? "sel" : ""}>{row.xtream}</td>
              <td className={kind === "m3u" ? "sel" : ""}>{row.m3u}</td>
            </tr>
          ))}
        </tbody>
      </table>

      {/* One form, so the labels share a column and the boxes line up. */}
      {kind === "xtream" ? (
        <div className="account-form">
          <div className="row-field">
            {/* "Xtream URL", not "panel address": it is the address of an
                Xtream Codes panel, and that is the phrase printed on the email
                a subscription arrives in. Somebody pasting a value in should
                be able to recognise which value it is being asked for. */}
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
      ) : (
        <div className="account-form">
          <div className="row-field">
            {/* A path is as good as a URL: plenty of playlists are a file
                somebody downloaded once and edits by hand. */}
            <label htmlFor="acct-playlist">Playlist</label>
            <input
              id="acct-playlist"
              className="field"
              value={m3u.playlist}
              spellCheck={false}
              autoComplete="off"
              placeholder={
                account?.playlistRedacted
                  ? `unchanged — ${account.playlist}`
                  : "https://… or /path/to/playlist.m3u"
              }
              onChange={(e) => setM3u({ ...m3u, playlist: e.target.value })}
            />
          </div>

          <div className="row-field">
            <label htmlFor="acct-epg">Guide (XMLTV)</label>
            <textarea
              id="acct-epg"
              className="field"
              rows={2}
              value={m3u.epgText}
              spellCheck={false}
              autoComplete="off"
              placeholder={
                account?.epgRedacted
                  ? `unchanged — ${account.epgUrls.join(", ")}`
                  : "optional; one address per line, .xml or .xml.gz"
              }
              onChange={(e) => setM3u({ ...m3u, epgText: e.target.value, epgTouched: true })}
            />
          </div>

          <div className="row-field">
            <label htmlFor="acct-ua">User agent</label>
            <input
              id="acct-ua"
              className="field"
              value={m3u.userAgent}
              spellCheck={false}
              autoComplete="off"
              placeholder="optional; only if the provider asks for one"
              onChange={(e) => setM3u({ ...m3u, userAgent: e.target.value })}
            />
          </div>
        </div>
      )}

      <div className="settings-actions">
        <button className="ctl primary" disabled={busy || !changed} onClick={() => void save()}>
          Save
        </button>
        {/* Tests what is saved, so it would test the other kind while the
            switch is showing this one. Save first. */}
        <button
          className="ctl"
          disabled={busy || switching}
          title={switching ? "Save first: the test checks the saved account" : undefined}
          onClick={() => void test()}
        >
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
        {kind === "m3u" ? (
          <p>
            With no guide address, zapper uses the one the playlist names in its header, if it
            names one. Leave a field showing “unchanged” empty to keep what is saved.
          </p>
        ) : null}
        <p>
          Stored in <code data-selectable>{account?.path ?? "—"}</code>, readable only by
          you.{" "}
          {kind === "m3u"
            ? "Credentials inside a playlist or guide address are never sent back to this window."
            : "The password is never sent back to this window."}
        </p>
      </div>
    </>
  );
}
