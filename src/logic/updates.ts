// What the update badge, dialog and settings pane say about the updater's
// state. The deciding all happens in Rust (src-tauri/src/updates/): which
// release is newer, which file this install takes, whether the download is
// the one SHA256SUMS vouches for. This file only turns that state into words
// and into the one thing the dialog's main button should do.
//
// Pure; checked in src/__checks__/updates.test.mjs.

import type { InstallKind, UpdateReleaseDto, UpdateStateDto, UpdateStatusDto } from "../types";

/** The release a state carries, if any. A failed check carries none. */
export function releaseOf(s: UpdateStateDto): UpdateReleaseDto | null {
  return "release" in s ? s.release : null;
}

/** Whether the toolbar shows its update badge. Only when there is something
 *  to act on: an update, its progress, or a download or install that went
 *  wrong. "Up to date" and "checking" are not worth a pixel of title bar. */
export function showsBadge(s: UpdateStateDto): boolean {
  return releaseOf(s) !== null;
}

/** 0–100, for the download's bar. */
export function percentOf(s: UpdateStateDto): number {
  if (s.state !== "downloading" || s.total <= 0) return 0;
  return Math.min(100, Math.floor((s.received * 100) / s.total));
}

/** `9.4 MB`, `640 KB`. */
export function formatSize(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

/** The badge's few words. */
export function badgeLabel(s: UpdateStateDto): string {
  switch (s.state) {
    case "available":
      return `Update to ${s.release.version}`;
    case "downloading":
      return `Downloading ${percentOf(s)}%`;
    case "ready":
      return "Update ready to install";
    case "installing":
      return "Installing update…";
    case "installed":
      return "Restart to update";
    case "failed":
      return "Update failed";
    default:
      return "";
  }
}

/** One line on where things stand, for the settings pane and the dialog. */
export function statusLine(st: UpdateStatusDto): string {
  const s = st.state;
  switch (s.state) {
    case "notConfigured":
      return "Updates aren't configured for this build: it doesn't know where zapper's releases are published.";
    case "idle":
      return "Not checked yet.";
    case "checking":
      return "Checking for updates…";
    case "upToDate":
      return `zapper ${st.current} is the newest version.`;
    case "available":
      return `zapper ${s.release.version} is available. You have ${st.current}.`;
    case "downloading":
      return `Downloading zapper ${s.release.version}…`;
    case "ready":
      return `zapper ${s.release.version} is downloaded and checked, ready to install.`;
    case "installing":
      return `Installing zapper ${s.release.version}…`;
    case "installed":
      return `zapper ${s.release.version} is installed. Restart zapper to start using it.`;
    case "failed":
      return s.message;
  }
}

/** What the dialog's main button does. */
export type UpdateStep =
  | "check"
  | "download"
  | "cancel"
  | "install"
  | "restart"
  | "page"
  | null;

export function nextStep(s: UpdateStateDto): UpdateStep {
  switch (s.state) {
    case "notConfigured":
    case "checking":
    case "installing":
      return null;
    case "idle":
    case "upToDate":
      return "check";
    case "available":
      // A build from source, or a release without this install's package:
      // there is nothing to download, only a page to read.
      return s.release.asset ? "download" : "page";
    case "downloading":
      return "cancel";
    case "ready":
      return "install";
    case "installed":
      return "restart";
    case "failed":
      if (!s.release) return "check";
      return s.release.asset ? "download" : "page";
  }
}

export function stepLabel(step: UpdateStep, s: UpdateStateDto): string {
  switch (step) {
    case "check":
      return "Check now";
    case "download":
      return s.state === "failed" ? "Try again" : "Download and install";
    case "cancel":
      return "Cancel";
    case "install":
      return "Install";
    case "restart":
      return "Restart zapper";
    case "page":
      return "Open the release page";
    case null:
      return "";
  }
}

/** The toast for an update nobody asked about: once per version, ever, this
 *  run. `announced` is the last version already announced (or seen in the
 *  dialog); keyed on that rather than on the previous state, because a state
 *  can pass through ones that carry no release — a check that failed, a
 *  restart of the backend — and comparing against those announced the same
 *  update again every day. */
export function announcement(next: UpdateStateDto, announced: string | null): string | null {
  if (next.state !== "available") return null;
  if (next.release.version === announced) return null;
  return `zapper ${next.release.version} is available — open the menu and choose Check for updates.`;
}

/** How this copy was installed, in words. */
export function kindLabel(kind: InstallKind): string {
  switch (kind) {
    case "deb":
      return "Debian/Ubuntu package";
    case "rpm":
      return "RPM package";
    case "nsis":
      return "Windows installer";
    case "msi":
      return "Windows installer (MSI)";
    case "dmg":
      return "macOS app";
    case "none":
      return "built from source";
  }
}

/** Whether installing needs the system's password prompt, worth saying
 *  before the button is pressed rather than after the prompt appears. */
export function asksForPassword(kind: InstallKind): boolean {
  return kind === "deb" || kind === "rpm";
}
