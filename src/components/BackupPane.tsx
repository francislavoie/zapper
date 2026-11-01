// Settings › Data's backups: export the settings, the category rail's choices
// and the watch history behind the favourites to a file, and import them
// back, after saying what would change.
//
// The format and every decision about it are in logic/backup.ts. The dialogs
// and the account are the backend's: this pane never sees a path or a
// password, only the file's name and what kind of account it holds.

import { useState } from "react";
import { backend } from "../backend";
import type { SettingsControl } from "../hooks/useSettings";
import { errorMessage } from "../logic/async";
import {
  backupFileName,
  buildBackup,
  parseBackup,
  planHasChanges,
  planImport,
  type Change,
  type ImportPlan,
} from "../logic/backup";
import { useApp, useDispatch } from "../store";
import type { BackupFileDto } from "../types";
import { SettingRow, SettingsGroup } from "./SettingsParts";
import { Switch } from "./Switch";

interface Pending {
  file: BackupFileDto;
  plan: ImportPlan;
  warnings: string[];
  pick: { settings: boolean; categories: boolean; favourites: boolean; account: boolean };
}

type Note = { text: string; bad: boolean } | null;

const KIND_LABEL: Record<string, string> = {
  xtream: "Xtream server",
  m3u: "M3U playlist",
  hdhomerun: "HDHomeRun",
  tvheadend: "Tvheadend server",
};

export function BackupPane({ s }: { s: SettingsControl }) {
  const { categoryPins, categoryHidden, categorySort } = useApp();
  const dispatch = useDispatch();
  const [withAccount, setWithAccount] = useState(false);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<Note>(null);
  const [pending, setPending] = useState<Pending | null>(null);

  const categories = { pins: [...categoryPins], hidden: [...categoryHidden], sort: categorySort };

  const exportNow = async () => {
    setBusy(true);
    setNote(null);
    try {
      const now = new Date();
      const doc = buildBackup(
        { settings: s.settings, categories, favourites: await backend.activityRows() },
        now,
      );
      const name = await backend.exportBackup(doc, withAccount, backupFileName(now));
      if (name !== null) setNote({ text: `Exported to ${name}.`, bad: false });
    } catch (e) {
      setNote({ text: `Export failed: ${errorMessage(e)}`, bad: true });
    } finally {
      setBusy(false);
    }
  };

  const openNow = async () => {
    setBusy(true);
    setNote(null);
    setPending(null);
    try {
      const file = await backend.openBackup();
      if (file === null) return;
      const parsed = parseBackup(file.document);
      if (!parsed.ok) {
        setNote({ text: `${file.name}: ${parsed.error}`, bad: true });
        return;
      }
      const plan = planImport(
        { settings: s.settings, categories, activity: await backend.activityRows() },
        parsed.backup,
      );
      const has = planHasChanges(plan);
      // Everything that would change is ticked, except the account: replacing
      // the one on file is a bigger step than the rest, and asked for apart.
      setPending({
        file,
        plan,
        warnings: parsed.warnings,
        pick: { ...has, account: false },
      });
    } catch (e) {
      setNote({ text: `Import failed: ${errorMessage(e)}`, bad: true });
    } finally {
      setBusy(false);
    }
  };

  const apply = async () => {
    if (!pending) return;
    const { plan, pick } = pending;
    setBusy(true);
    const done: string[] = [];
    try {
      if (pick.settings && plan.settings) {
        // Through the settings pane's own save, which applies the playback
        // and subtitle changes to mpv as well as storing them.
        s.patch(plan.settings.next);
        done.push("settings");
      }
      if (pick.categories && plan.categories) {
        dispatch({ type: "categoryPrefs", ...plan.categories.next });
        done.push("categories");
      }
      if (pick.favourites && plan.favourites) {
        await backend.importActivity(plan.favourites.rows);
        dispatch({ type: "activityChanged" });
        done.push("favourites");
      }
      if (pick.account && pending.file.account) {
        await backend.importBackupAccount();
        done.push("the account");
      }
      setPending(null);
      setNote({
        text: done.length ? `Imported ${joinWords(done)} from ${pending.file.name}.` : "Nothing imported.",
        bad: false,
      });
    } catch (e) {
      const sofar = done.length ? ` (${joinWords(done)} already imported)` : "";
      setNote({ text: `Import failed: ${errorMessage(e)}${sofar}`, bad: true });
    } finally {
      setBusy(false);
    }
  };

  const toggle = (k: keyof Pending["pick"]) =>
    setPending((p) => (p ? { ...p, pick: { ...p.pick, [k]: !p.pick[k] } } : p));

  return (
    <SettingsGroup heading="Back up and restore">
      <SettingRow
        label="Export"
        htmlFor="backup-export"
        description="Settings, favourites and category choices, to a file."
      >
        <button id="backup-export" className="ctl" disabled={busy} onClick={() => void exportNow()}>
          Export…
        </button>
      </SettingRow>
      <SettingRow
        label="Include the account"
        htmlFor="backup-account"
        description={
          withAccount ? (
            <span className="set-warn">
              The file will contain the account’s password, and any login in its playlist or
              guide addresses, as plain text. Keep it somewhere private.
            </span>
          ) : (
            "Its password too, so the file can set up zapper on another machine."
          )
        }
      >
        <Switch id="backup-account" checked={withAccount} onChange={setWithAccount} />
      </SettingRow>
      <SettingRow
        label="Restore from a file"
        htmlFor="backup-import"
        description="Shows what the file would change, and changes nothing until you apply."
      >
        <button id="backup-import" className="ctl" disabled={busy} onClick={() => void openNow()}>
          Import…
        </button>
      </SettingRow>

      {note || pending ? (
        <div className="set-card-foot">
          {note ? (
            <p className={`settings-note ${note.bad ? "bad" : ""}`} role="status">
              {note.text}
            </p>
          ) : null}
          {pending ? (
            <ImportPreview pending={pending} busy={busy} toggle={toggle} apply={() => void apply()} cancel={() => setPending(null)} />
          ) : null}
        </div>
      ) : null}
    </SettingsGroup>
  );
}

function ImportPreview({
  pending,
  busy,
  toggle,
  apply,
  cancel,
}: {
  pending: Pending;
  busy: boolean;
  toggle: (k: keyof Pending["pick"]) => void;
  apply: () => void;
  cancel: () => void;
}) {
  const { file, plan, pick, warnings } = pending;
  const fav = plan.favourites;
  const nothing =
    !(plan.settings?.changes.length || plan.categories?.changes.length) &&
    !(fav && fav.added + fav.raised > 0) &&
    !file.account;
  const any = pick.settings || pick.categories || pick.favourites || pick.account;

  return (
    <div className="backup-preview" aria-label={`Import from ${file.name}`}>
      <p>
        From <b>{file.name}</b>. Tick what to import; nothing changes until
        you apply.
      </p>

      {plan.settings ? (
        <Section
          id="settings"
          heading="Settings"
          checked={pick.settings}
          onToggle={toggle}
          changes={plan.settings.changes}
        />
      ) : null}
      {plan.categories ? (
        <Section
          id="categories"
          heading="Categories"
          checked={pick.categories}
          onToggle={toggle}
          changes={plan.categories.changes}
        />
      ) : null}
      {fav ? (
        <Section
          id="favourites"
          heading="Favourites"
          checked={pick.favourites}
          onToggle={toggle}
          summary={
            fav.added + fav.raised === 0
              ? null
              : `Watch history for ${fav.rows.length.toLocaleString()} channels: ${fav.added.toLocaleString()} new here, ${fav.raised.toLocaleString()} with more watching than here. Nothing watched here is lowered.`
          }
        />
      ) : null}
      {file.account ? (
        <Section
          id="account"
          heading="Account"
          checked={pick.account}
          onToggle={toggle}
          summary={`Replaces the account on file with the ${KIND_LABEL[file.account.kind] ?? file.account.kind} ${file.account.description}${file.account.hasSecret ? ", password included" : ""}, and refreshes the catalogue from it.`}
        />
      ) : null}
      {file.accountError ? (
        <p className="settings-note bad">The file’s account is left out: {file.accountError}.</p>
      ) : null}
      {warnings.length ? (
        <ul className="backup-warnings">
          {warnings.map((w) => (
            <li key={w}>{w}</li>
          ))}
        </ul>
      ) : null}

      {nothing ? <p>This file matches what is here already.</p> : null}
      <div className="backup-actions">
        <button className="ctl" onClick={cancel} disabled={busy}>
          Cancel
        </button>
        <button className="ctl on" onClick={apply} disabled={busy || !any}>
          Apply
        </button>
      </div>
    </div>
  );
}

function Section({
  id,
  heading,
  checked,
  onToggle,
  changes,
  summary,
}: {
  id: keyof Pending["pick"];
  heading: string;
  checked: boolean;
  onToggle: (k: keyof Pending["pick"]) => void;
  changes?: Change[];
  summary?: string | null;
}) {
  const empty = changes ? changes.length === 0 : summary === null;
  return (
    <div className={`backup-section ${empty ? "same" : ""}`}>
      <div className="backup-pick">
        <label htmlFor={`backup-pick-${id}`}>
          {heading}
          {empty ? <span className="backup-same"> — no changes</span> : null}
        </label>
        <Switch
          id={`backup-pick-${id}`}
          checked={checked}
          disabled={empty}
          onChange={() => onToggle(id)}
        />
      </div>
      {changes && changes.length ? (
        <dl className="backup-changes">
          {changes.map((c) => (
            <div key={c.label}>
              <dt>{c.label}</dt>
              <dd>
                <span className="from">{c.from}</span> → {c.to}
              </dd>
            </div>
          ))}
        </dl>
      ) : null}
      {summary ? <p>{summary}</p> : null}
    </div>
  );
}

function joinWords(xs: string[]): string {
  if (xs.length <= 1) return xs.join("");
  return `${xs.slice(0, -1).join(", ")} and ${xs[xs.length - 1]}`;
}
