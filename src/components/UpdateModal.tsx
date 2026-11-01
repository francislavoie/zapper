// The update dialog: what is installed, what is available, and one button
// for the next step. Opened by "Check for updates" in the menu or the
// palette, and by the badge in the title bar.
//
// Every step is a request to the backend, which decides what to fetch and
// install from its own state; this only says what is happening. The steps
// are download (with progress), install (which on Linux raises the system's
// password prompt), then restart. "Download and install" is one press: once
// the download has been checked against the release's SHA256SUMS the
// install starts by itself, so nobody is asked to press a second button for
// the thing they just asked for.

import { useEffect, useRef, useState } from "react";
import { backend } from "../backend";
import { checkForUpdates } from "../actions";
import { errorMessage } from "../logic/async";
import { dateTime } from "../logic/format";
import {
  asksForPassword,
  aurNote,
  formatSize,
  kindLabel,
  nextStep,
  percentOf,
  recordingPrompt,
  releaseOf,
  statusLine,
  stepLabel,
  type UpdateStep,
} from "../logic/updates";
import { useApp, useDispatch } from "../store";
import type { InstallOutcomeDto, InstallWhen } from "../types";

export function UpdateModal() {
  const { update } = useApp();
  const dispatch = useDispatch();
  const close = () => dispatch({ type: "updateDialog", open: false });
  const [error, setError] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<InstallOutcomeDto | null>(null);
  const [copied, setCopied] = useState(false);
  /** A request is on its way. The buttons that act are disabled until it
   *  answers: a double click on Install must not become two package manager
   *  runs (the backend refuses the second too, but should not have to). */
  const [pending, setPending] = useState(false);
  /** Set by "Download and install"; the install follows the verified
   *  download without a second press. */
  const installWhenReady = useRef(false);

  const s = update.state;
  const release = releaseOf(s);
  const step = nextStep(s);

  const install = async (when: InstallWhen = "ask") => {
    setError(null);
    setOutcome(null);
    setPending(true);
    try {
      setOutcome(await backend.updateInstall(when));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setPending(false);
    }
  };

  useEffect(() => {
    if (s.state === "ready" && installWhenReady.current) {
      installWhenReady.current = false;
      void install();
    }
    if (s.state === "failed" || s.state === "available") installWhenReady.current = false;
    // Keyed on the state's name alone: progress ticks change `s` many times
    // a second and none of them is a reason to look again.
  }, [s.state]);

  const act = async (what: UpdateStep) => {
    // Cancel answers even while a request is out: waiting for a recording to
    // finish is an install request that stays out until it does.
    if (pending && what !== "cancel") return;
    if (what === "install") return install();
    setError(null);
    if (what !== "cancel") setPending(true);
    try {
      switch (what) {
        case "check":
          await checkForUpdates(dispatch);
          break;
        case "download":
          setOutcome(null);
          installWhenReady.current = true;
          dispatch({ type: "update", value: await backend.updateDownload() });
          break;
        case "cancel":
          installWhenReady.current = false;
          dispatch({ type: "update", value: await backend.updateCancel() });
          break;
        case "restart":
          await backend.updateRestart();
          break;
        case "page":
          await backend.updateOpenPage();
          break;
        case null:
          break;
      }
    } catch (e) {
      installWhenReady.current = false;
      setError(errorMessage(e));
    } finally {
      if (what !== "cancel") setPending(false);
    }
  };

  const copy = (text: string) => {
    void navigator.clipboard?.writeText(text).then(
      () => setCopied(true),
      () => setCopied(false),
    );
  };

  const busy =
    pending || s.state === "checking" || s.state === "installing" || s.state === "waitingForRecording";
  const asking = outcome?.outcome === "recording" ? recordingPrompt(outcome.until, outcome.title) : null;

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) close();
      }}
    >
      <div className="about update" role="dialog" aria-label="Updates">
        <h2>{release ? release.name : "Updates"}</h2>
        <p className={`about-tag ${s.state === "failed" ? "bad" : ""}`} role="status" aria-live="polite">
          {busy ? <span className="tb-spinner" aria-hidden /> : null}
          {statusLine(update)}
        </p>

        <dl className="about-facts">
          <div>
            <dt>Installed</dt>
            <dd>
              {update.current || "unknown"} · {kindLabel(update.kind)}
            </dd>
          </div>
          {update.lastChecked ? (
            <div>
              <dt>Checked</dt>
              <dd>{dateTime(update.lastChecked)}</dd>
            </div>
          ) : null}
          {release?.publishedAt ? (
            <div>
              <dt>Released</dt>
              <dd>
                {new Date(release.publishedAt).toLocaleDateString()}
                {release.prerelease ? " · pre-release" : ""}
              </dd>
            </div>
          ) : null}
          {release?.asset ? (
            <div>
              <dt>Download</dt>
              <dd>
                {release.asset.name} · {formatSize(release.asset.size)}
              </dd>
            </div>
          ) : null}
        </dl>

        {s.state === "downloading" ? (
          <div
            className="update-progress"
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={percentOf(s)}
          >
            <div style={{ width: `${percentOf(s)}%` }} />
          </div>
        ) : null}

        {release && release.notes.trim() !== "" ? (
          <pre className="update-notes">{release.notes.trim()}</pre>
        ) : null}

        {release && !release.asset && update.kind === "none" ? (
          <div className="settings-note">
            This copy was built from source, so it can’t replace itself. The
            release page has the packages.
          </div>
        ) : null}

        {step === "download" && asksForPassword(update.kind) ? (
          <div className="settings-note">
            Installing asks for your password: the package is installed with
            the system’s package manager, like any other. The prompt names{" "}
            <code>/bin/sh</code> — a short script that first copies the package
            where only the system can change it and checks it again, so what
            gets installed is exactly what was verified.
          </div>
        ) : null}

        {step === "download" && aurNote(update.kind) ? (
          <div className="settings-note">{aurNote(update.kind)}</div>
        ) : null}

        {outcome?.outcome === "manual" ? (
          <div className="settings-note bad">
            <p className="update-reason">{outcome.reason} Run this in a terminal instead:</p>
            <code className="update-command">{outcome.command}</code>
            <button className="link" onClick={() => copy(outcome.command)}>
              {copied ? "copied" : "copy"}
            </button>
            {outcome.output ? <pre className="update-output">{outcome.output}</pre> : null}
          </div>
        ) : null}

        {asking ? <div className="settings-note">{asking.message}</div> : null}

        {error ? <div className="settings-note bad">{error}</div> : null}

        <div className="about-actions update-actions">
          {release && step !== "page" ? (
            <button className="ctl ghost" disabled={pending} onClick={() => void act("page")}>
              Release page
            </button>
          ) : null}
          {asking ? (
            <>
              <button className="ctl" disabled={pending} onClick={() => void install("now")}>
                {asking.now}
              </button>
              <button className="ctl on" disabled={pending} onClick={() => void install("whenIdle")} autoFocus>
                {asking.wait}
              </button>
            </>
          ) : step ? (
            <button
              className="ctl on"
              // Cancel stays live during a download: it is the one thing a
              // pending download must still answer to.
              disabled={pending && step !== "cancel"}
              onClick={() => void act(step)}
              autoFocus
            >
              {stepLabel(step, s)}
            </button>
          ) : null}
          <button className="ctl" onClick={close} autoFocus={!step}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
