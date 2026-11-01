// The license notices Settings > About lists: what each file the build writes into
// dist/licenses/ is called there, and the order they come in (tools/licenses.mjs writes them,
// docs/licensing.md says what each holds).
//
// Pure, so the node checks can hold the list to the files the build actually writes.

/** A notice file as dist/licenses/index.json lists it. */
export interface NoticeFile {
  name: string;
  size: number;
}

/** Each file's name in the list, and the line under it saying what it covers. The order here is
 *  the list's: zapper's own license first, then what the player is built on, then the long
 *  generated lists. */
export const NOTICE_LABELS: Record<string, { label: string; detail: string }> = {
  "LICENSE.txt": { label: "zapper", detail: "MIT License" },
  "MPV-FFMPEG-NOTICE.txt": { label: "mpv and FFmpeg", detail: "playback, through libmpv" },
  "GPL-3.0.txt": { label: "GNU GPL, version 3", detail: "libmpv-2.dll's license" },
  "THIRD-PARTY-NOTICES-rust.txt": { label: "Rust crates", detail: "the app's native code" },
  "THIRD-PARTY-NOTICES-ui.txt": { label: "Interface", detail: "npm packages" },
};

const ORDER = Object.keys(NOTICE_LABELS);

const rank = (name: string): number => {
  const i = ORDER.indexOf(name);
  return i < 0 ? ORDER.length : i;
};

/** The files in the list's order; one this list doesn't know goes last, by name, under its own
 *  name, rather than being hidden. */
export function orderNotices(files: readonly NoticeFile[]): NoticeFile[] {
  return [...files].sort((a, b) => rank(a.name) - rank(b.name) || a.name.localeCompare(b.name));
}

/** What the list calls a file. */
export function noticeLabel(name: string): { label: string; detail: string | null } {
  const known = NOTICE_LABELS[name];
  return known ? known : { label: name, detail: null };
}

/** Whether a fetched notice is really the app's index.html: a path the app doesn't have is
 *  answered with the page (Tauri's asset protocol, and Vite's dev server), not a 404, so a
 *  missing file would otherwise show up as a screenful of HTML. */
export function isPageFallback(contentType: string | null, body: string): boolean {
  return (contentType ?? "").includes("text/html") || /^\s*<!doctype html/i.test(body);
}
