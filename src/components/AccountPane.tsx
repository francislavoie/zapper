// The account: an Xtream Codes login, an M3U playlist with an optional guide,
// an HDHomeRun tuner, or a Tvheadend server.
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
// A Tvheadend password follows the Xtream password's rule exactly. An HDHomeRun has no
// login at all — it is a box on the LAN — and its optional XMLTV guides
// follow the playlist's guides' rule.
//
// The two kinds are not equivalent, and the pane says how before anyone
// picks one: each type's card lists what it needs and what it gets, kept as
// data in logic/sources.ts so it is checked against what the backend does.

import { useEffect, useState } from "react";
import { backend, capabilities, passwordsInClear } from "../backend";
import { errorMessage } from "../logic/async";
import {
  SOURCE_KINDS,
  asKind,
  candidateFor,
  hdhrChanged,
  hdhrFormFrom,
  kindInfo,
  m3uChanged,
  m3uFormFrom,
  planHdhrSave,
  planM3uSave,
  planTvhSave,
  tvhChanged,
  tvhFormFrom,
  type HdhrForm,
  type M3uForm,
  type SourceKind,
  type TvhForm,
} from "../logic/sources";
import type { AccountDto } from "../types";
import { SettingRow, SettingsGroup } from "./SettingsParts";

const EMPTY_M3U: M3uForm = { playlist: "", epgText: "", userAgent: "", epgTouched: false };
const EMPTY_HDHR: HdhrForm = { device: "", epgText: "", epgTouched: false };
const EMPTY_TVH: TvhForm = { url: "", username: "", password: "" };

export function AccountPane() {
  const [account, setAccount] = useState<AccountDto | null>(null);
  const [kind, setKind] = useState<SourceKind>("xtream");
  const [host, setHost] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [m3u, setM3u] = useState<M3uForm>(EMPTY_M3U);
  const [hdhr, setHdhr] = useState<HdhrForm>(EMPTY_HDHR);
  const [tvh, setTvh] = useState<TvhForm>(EMPTY_TVH);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<{ kind: "ok" | "bad"; text: string } | null>(null);

  const adopt = (a: AccountDto) => {
    setAccount(a);
    setHost(a.host);
    setUsername(a.username);
    setM3u(m3uFormFrom(a));
    setHdhr(hdhrFormFrom(a));
    // Which also empties the password field, as the Xtream one is emptied.
    setTvh(tvhFormFrom(a));
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
      } else if (kind === "hdhomerun") {
        if (!account) return;
        const plan = planHdhrSave(hdhr, account);
        if ("error" in plan) throw plan.error;
        await backend.setHdhomerunAccount(plan.device, plan.epgUrls);
      } else if (kind === "tvheadend") {
        if (!account) return;
        const plan = planTvhSave(tvh, account);
        if ("error" in plan) throw plan.error;
        await backend.setTvheadendAccount(plan.url, plan.username, plan.password);
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

  // Tests the form, not the file: a change is tried before it is saved, and
  // so is a kind that is not in use yet. Blank secret fields stand for what
  // is on file, as they do for Save, so the answer is about what Save would
  // keep.
  const test = async () => {
    if (!account) return;
    setBusy(true);
    setNote(null);
    try {
      const candidate = candidateFor(
        kind,
        { xtream: { host, username, password }, m3u, hdhr, tvh },
        account,
      );
      if ("error" in candidate) throw candidate.error;
      setNote({ kind: "ok", text: await backend.testAccountCandidate(candidate) });
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
  const fieldsChanged = (a: AccountDto): boolean => {
    switch (kind) {
      case "xtream":
        return host.trim() !== a.host || username.trim() !== a.username || password !== "";
      case "m3u":
        return m3uChanged(m3u, a);
      case "hdhomerun":
        return hdhrChanged(hdhr, a);
      case "tvheadend":
        return tvhChanged(tvh, a);
    }
  };
  const changed = account !== null && (switching || fieldsChanged(account));

  // "This window" in the app; in a browser, the page the service sent.
  const here = capabilities.window ? "this window" : "this page";

  return (
    <>
      {/* Said before anything is typed, since typing it is the risk. */}
      {passwordsInClear ? (
        <div className="settings-note bad" role="note">
          This page uses plain HTTP, so a password saved or tested here crosses the network
          unencrypted. To encrypt it, turn off “Plain HTTP on the network” in the zapper app’s
          Settings → Recording.
        </div>
      ) : null}
      {/* The choice is the explanation: one card per type, saying what you
          need for it and what you get from it, so picking one and learning
          what it means are the same act. */}
      <SettingsGroup heading="Account type" bare>
        <div className="source-cards" role="radiogroup" aria-label="Account type">
          {SOURCE_KINDS.map((k) => {
            const on = kind === k.id;
            return (
              <button
                key={k.id}
                role="radio"
                aria-checked={on}
                className={`source-card ${on ? "on" : ""}`}
                onClick={() => {
                  setKind(k.id);
                  setNote(null);
                }}
              >
                <span className="source-card-head">
                  <span className="source-card-dot" aria-hidden />
                  <span className="source-card-title">{k.label}</span>
                </span>
                <span className="source-card-needs">Needs: {k.needs}</span>
                <span className="source-card-points">
                  {k.points.map((p) => (
                    <span key={p.text} className={`source-point ${p.ok ? "ok" : "limit"}`}>
                      <span className="source-point-mark" aria-label={p.ok ? "yes" : "limit"}>
                        {p.ok ? "✓" : "–"}
                      </span>
                      {p.text}
                    </span>
                  ))}
                </span>
              </button>
            );
          })}
        </div>
        {switching ? (
          <p className="source-switching">
            Saving switches the account type to {kindInfo(kind).label}.
          </p>
        ) : null}
      </SettingsGroup>

      {/* One box of form rows, so the labels share a column and the inputs
          line up down their left edge. */}
      <SettingsGroup heading={kindInfo(kind).label}>
        {kind === "xtream" ? (
          <>
            {/* "Xtream URL": it is the address of an Xtream Codes server, and
                that is how providers usually label it when they hand it out.
                Somebody pasting a value in should be able to recognise which
                value it is being asked for. */}
            <SettingRow label="Xtream URL" htmlFor="acct-host" layout="form">
              <input
                id="acct-host"
                className="field"
                value={host}
                spellCheck={false}
                autoComplete="off"
                placeholder="http://tv.example.com:8080"
                onChange={(e) => setHost(e.target.value)}
              />
            </SettingRow>
            <SettingRow label="Username" htmlFor="acct-user" layout="form">
              <input
                id="acct-user"
                className="field"
                value={username}
                spellCheck={false}
                autoComplete="off"
                onChange={(e) => setUsername(e.target.value)}
              />
            </SettingRow>
            <SettingRow label="Password" htmlFor="acct-pass" layout="form">
              <input
                id="acct-pass"
                className="field"
                type="password"
                value={password}
                autoComplete="off"
                placeholder={account?.hasPassword ? "unchanged" : "required"}
                onChange={(e) => setPassword(e.target.value)}
              />
            </SettingRow>
          </>
        ) : kind === "hdhomerun" ? (
          <>
            {/* The address, not a name from a list: zapper does not scan the
                network, and the HDHomeRun app shows the address on its
                device page. */}
            <SettingRow label="Tuner address" htmlFor="acct-hdhr" layout="form">
              <input
                id="acct-hdhr"
                className="field"
                value={hdhr.device}
                spellCheck={false}
                autoComplete="off"
                placeholder="192.168.1.50 or hdhomerun.local"
                onChange={(e) => setHdhr({ ...hdhr, device: e.target.value })}
              />
            </SettingRow>
            <SettingRow label="Guide (XMLTV)" htmlFor="acct-hdhr-epg" layout="form">
              <textarea
                id="acct-hdhr-epg"
                className="field"
                rows={2}
                value={hdhr.epgText}
                spellCheck={false}
                autoComplete="off"
                placeholder={
                  account?.hdhomerunEpgRedacted
                    ? `unchanged — ${account.hdhomerunEpgUrls.join(", ")}`
                    : "optional; leave empty for SiliconDust's free guide"
                }
                onChange={(e) => setHdhr({ ...hdhr, epgText: e.target.value, epgTouched: true })}
              />
            </SettingRow>
          </>
        ) : kind === "tvheadend" ? (
          <>
            <SettingRow label="Server" htmlFor="acct-tvh" layout="form">
              <input
                id="acct-tvh"
                className="field"
                value={tvh.url}
                spellCheck={false}
                autoComplete="off"
                placeholder="http://tvheadend.local:9981"
                onChange={(e) => setTvh({ ...tvh, url: e.target.value })}
              />
            </SettingRow>
            <SettingRow label="Username" htmlFor="acct-tvh-user" layout="form">
              <input
                id="acct-tvh-user"
                className="field"
                value={tvh.username}
                spellCheck={false}
                autoComplete="off"
                placeholder="optional"
                onChange={(e) => setTvh({ ...tvh, username: e.target.value })}
              />
            </SettingRow>
            <SettingRow label="Password" htmlFor="acct-tvh-pass" layout="form">
              <input
                id="acct-tvh-pass"
                className="field"
                type="password"
                value={tvh.password}
                autoComplete="off"
                placeholder={
                  account?.tvheadendHasPassword
                    ? "unchanged"
                    : tvh.username.trim() === ""
                      ? "optional"
                      : "required"
                }
                onChange={(e) => setTvh({ ...tvh, password: e.target.value })}
              />
            </SettingRow>
          </>
        ) : (
          <>
            {/* A path is as good as a URL: plenty of playlists are a file
                somebody downloaded once and edits by hand. */}
            <SettingRow label="Playlist" htmlFor="acct-playlist" layout="form">
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
            </SettingRow>
            <SettingRow label="Guide (XMLTV)" htmlFor="acct-epg" layout="form">
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
            </SettingRow>
            <SettingRow label="User agent" htmlFor="acct-ua" layout="form">
              <input
                id="acct-ua"
                className="field"
                value={m3u.userAgent}
                spellCheck={false}
                autoComplete="off"
                placeholder="optional; only if the provider asks for one"
                onChange={(e) => setM3u({ ...m3u, userAgent: e.target.value })}
              />
            </SettingRow>
          </>
        )}

        <div className="set-card-foot">
          <div className="settings-actions">
            <button className="ctl primary" disabled={busy || !changed} onClick={() => void save()}>
              Save
            </button>
            {/* Tests what is in the fields, saved or not; nothing is saved by
                testing. */}
            <button className="ctl" disabled={busy || !account} onClick={() => void test()}>
              Test connection
            </button>
          </div>
          {/* Boxed, because it is news: it is the answer to the button that
              was just pressed, and it goes away again. */}
          {note ? (
            <p className={`settings-note ${note.kind === "bad" ? "bad" : ""}`} role="status">
              {note.text}
            </p>
          ) : null}
        </div>
      </SettingsGroup>

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
        {kind === "hdhomerun" ? (
          <p>
            Each channel needs a free tuner while it plays. Channels the tuner marks as
            copy-protected cannot play here, so they are left out of the list.
          </p>
        ) : null}
        {/* The one setting a Tvheadend user may have to change, said before
            the test fails rather than only after. */}
        {kind === "tvheadend" ? (
          <p>
            With a login, Tvheadend must give that user persistent authentication: in its web
            interface, Configuration → Users → Passwords, set “Persistent authentication” to
            Enable. Leave both login fields empty if the server lets anyone watch.
          </p>
        ) : null}
        <p>
          {/* The recording service does not say where on its computer the
              file is: the browser may be on another one. */}
          {account && !account.path ? (
            "Stored by the recording service, on its computer, readable only by its user."
          ) : (
            <>
              Stored in <code data-selectable>{account?.path ?? "—"}</code>, readable only by
              you.
            </>
          )}{" "}
          {kind === "m3u"
            ? `Credentials inside a playlist or guide address are never sent back to ${here}.`
            : kind === "hdhomerun"
              ? `Credentials inside a guide address are never sent back to ${here}.`
              : `The password is never sent back to ${here}.`}
        </p>
      </div>
    </>
  );
}
