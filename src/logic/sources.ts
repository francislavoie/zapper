// The two kinds of account, what each can do, and the playlist form's rules.
//
// What each kind can do is data rather than markup so it can be checked: it
// is a promise about what the backend does, and a promise kept in JSX drifts
// the first time the backend changes and nobody rereads the settings pane.
// Each point says what zapper actually does with that kind of account today
// — not what the format could do in principle.
//
// Pure; checked in src/__checks__/sources.test.mjs.

import type { AccountDto } from "../types";

export type SourceKind = "xtream" | "m3u";

/** One thing an account type does — or, with `ok: false`, does not do. */
export interface SourcePoint {
  text: string;
  ok: boolean;
}

export interface SourceKindInfo {
  id: SourceKind;
  label: string;
  /** What you need to have to hand, in the words a provider's email uses. */
  needs: string;
  /** What you get, in a few short lines. Limits are listed too, marked as
   *  such, because finding out after setting one up is finding out late. */
  points: readonly SourcePoint[];
}

// Each type is described on its own card rather than in a side-by-side
// table. A table would need a toggle, a sentence about one side and a
// five-row grid about both to be read together before anything made sense;
// a card per type says the same things where the choice is made.
export const SOURCE_KINDS: readonly SourceKindInfo[] = [
  {
    id: "xtream",
    label: "Xtream panel",
    needs: "Server address, username and password",
    points: [
      { text: "Channels and guide from one login", ok: true },
      { text: "Catch-up wherever your provider offers it", ok: true },
      { text: "Shows account status and connections in use", ok: true },
    ],
  },
  {
    id: "m3u",
    label: "M3U playlist",
    needs: "A playlist link or file",
    points: [
      { text: "Categories from the playlist's groups", ok: true },
      { text: "Guide only if you add an XMLTV link", ok: false },
      { text: "Catch-up only where the playlist marks it", ok: false },
      { text: "No account status or expiry", ok: false },
    ],
  },
];

export function kindInfo(kind: SourceKind): SourceKindInfo {
  return SOURCE_KINDS.find((k) => k.id === kind) ?? SOURCE_KINDS[0];
}

/** The backend's `kind`, read defensively: anything unknown is Xtream,
 *  which is what an account file with no kind in it means. */
export function asKind(raw: string | undefined | null): SourceKind {
  return raw === "m3u" ? "m3u" : "xtream";
}

/** Guide addresses as typed: one per line, or a comma-separated list the way
 *  a playlist's `x-tvg-url` writes them. Commas only split where the next
 *  thing looks like the start of an address, so a comma inside a URL's
 *  query string stays put. Blanks and repeats are dropped. */
export function splitUrls(text: string): string[] {
  const out: string[] = [];
  for (const part of text.split(/\r?\n|,(?=\s*(?:[a-z][a-z0-9+.-]*:\/\/|\/|~))/i)) {
    const t = part.trim();
    if (t !== "" && !out.includes(t)) out.push(t);
  }
  return out;
}

/** What the playlist fields hold while being edited. */
export interface M3uForm {
  playlist: string;
  epgText: string;
  userAgent: string;
  /** Whether the guide field has been typed in. A redacted list on file is
   *  kept unless it has. */
  epgTouched: boolean;
}

/** The arguments `set_m3u_account` takes. `null` keeps what is on file. */
export interface M3uSave {
  playlist: string | null;
  epgUrls: string[] | null;
  userAgent: string;
}

type M3uOnFile = Pick<
  AccountDto,
  "playlist" | "playlistRedacted" | "epgUrls" | "epgRedacted" | "userAgent"
>;

/** The form as it opens. A value with credentials redacted out of it is not
 *  put in the field — it could not be saved back as it stands — so the field
 *  starts empty, with the redacted form shown as its placeholder, exactly as
 *  the password field does. */
export function m3uFormFrom(a: M3uOnFile): M3uForm {
  return {
    playlist: a.playlistRedacted ? "" : a.playlist,
    epgText: a.epgRedacted ? "" : a.epgUrls.join("\n"),
    userAgent: a.userAgent,
    epgTouched: false,
  };
}

/** What a save would send, or why it cannot. */
export function planM3uSave(form: M3uForm, a: M3uOnFile): M3uSave | { error: string } {
  const typed = form.playlist.trim();
  let playlist: string | null;
  if (typed !== "") playlist = typed;
  else if (a.playlist !== "") playlist = null;
  else return { error: "a playlist URL or file is required" };

  const epgUrls = a.epgRedacted && !form.epgTouched ? null : splitUrls(form.epgText);
  return { playlist, epgUrls, userAgent: form.userAgent.trim() };
}

/** Whether there is anything to save: a field differs from what is on file. */
export function m3uChanged(form: M3uForm, a: M3uOnFile): boolean {
  const start = m3uFormFrom(a);
  return (
    form.playlist.trim() !== start.playlist ||
    (form.epgTouched && splitUrls(form.epgText).join("\n") !== splitUrls(start.epgText).join("\n")) ||
    form.userAgent.trim() !== start.userAgent
  );
}
