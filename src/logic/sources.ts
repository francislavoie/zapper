// The kinds of account, what each can do, and the forms' keep-or-replace
// rules.
//
// What each kind can do is data rather than markup so it can be checked: it
// is a promise about what the backend does, and a promise kept in JSX drifts
// the first time the backend changes and nobody rereads the settings pane.
// Each point says what zapper actually does with that kind of account today
// — not what the format could do in principle.
//
// Pure; checked in src/__checks__/sources.test.mjs.

import type { AccountDto } from "../types";

export type SourceKind = "xtream" | "m3u" | "hdhomerun" | "tvheadend";

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
    // "Xtream", as people say it, and the server is called a server. The
    // needs line names "Xtream Codes" in full, since that is what a
    // provider's email calls the login.
    label: "Xtream",
    needs: "An Xtream Codes server, username and password",
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
  {
    id: "hdhomerun",
    label: "HDHomeRun",
    needs: "An HDHomeRun tuner on your network",
    points: [
      { text: "Channels from the tuner's own lineup", ok: true },
      // SiliconDust's guide is free to owners and needs nothing typed in.
      { text: "Free guide from SiliconDust, or your XMLTV", ok: true },
      // A tuner plays one channel per tuner, and has no archive.
      { text: "No catch-up; one channel per free tuner", ok: false },
      { text: "Copy-protected channels are not listed", ok: false },
    ],
  },
  {
    id: "tvheadend",
    // Tvheadend's own spelling.
    label: "Tvheadend",
    needs: "A Tvheadend server; login optional",
    points: [
      { text: "Channels, tags and guide from the server", ok: true },
      // Without it the server will not hand out stream links that last;
      // the connection test says exactly where to switch it on.
      { text: "A login needs persistent auth turned on", ok: false },
      { text: "No catch-up or recordings", ok: false },
    ],
  },
];

export function kindInfo(kind: SourceKind): SourceKindInfo {
  return SOURCE_KINDS.find((k) => k.id === kind) ?? SOURCE_KINDS[0];
}

/** The backend's `kind`, read defensively: anything unknown is Xtream,
 *  which is what an account file with no kind in it means. */
export function asKind(raw: string | undefined | null): SourceKind {
  return SOURCE_KINDS.find((k) => k.id === raw)?.id ?? "xtream";
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

/** What the HDHomeRun fields hold while being edited. */
export interface HdhrForm {
  device: string;
  epgText: string;
  /** As for the playlist's guides: a redacted list is kept unless typed in. */
  epgTouched: boolean;
}

/** The arguments `set_hdhomerun_account` takes. `null` keeps the guides on
 *  file; an empty list means SiliconDust's guide. */
export interface HdhrSave {
  device: string;
  epgUrls: string[] | null;
}

type HdhrOnFile = Pick<AccountDto, "hdhomerunDevice" | "hdhomerunEpgUrls" | "hdhomerunEpgRedacted">;

export function hdhrFormFrom(a: HdhrOnFile): HdhrForm {
  return {
    device: a.hdhomerunDevice,
    epgText: a.hdhomerunEpgRedacted ? "" : a.hdhomerunEpgUrls.join("\n"),
    epgTouched: false,
  };
}

export function planHdhrSave(form: HdhrForm, a: HdhrOnFile): HdhrSave | { error: string } {
  const device = form.device.trim();
  if (device === "") return { error: "an HDHomeRun address is required" };
  const epgUrls = a.hdhomerunEpgRedacted && !form.epgTouched ? null : splitUrls(form.epgText);
  return { device, epgUrls };
}

export function hdhrChanged(form: HdhrForm, a: HdhrOnFile): boolean {
  const start = hdhrFormFrom(a);
  return (
    form.device.trim() !== start.device ||
    (form.epgTouched && splitUrls(form.epgText).join("\n") !== splitUrls(start.epgText).join("\n"))
  );
}

/** What the Tvheadend fields hold while being edited. The password starts
 *  empty whatever is on file — it is write-only, as the Xtream one is. */
export interface TvhForm {
  url: string;
  username: string;
  password: string;
}

export interface TvhSave {
  url: string;
  username: string;
  /** Empty keeps the one on file. */
  password: string;
}

type TvhOnFile = Pick<AccountDto, "tvheadendUrl" | "tvheadendUsername" | "tvheadendHasPassword">;

export function tvhFormFrom(a: TvhOnFile): TvhForm {
  return { url: a.tvheadendUrl, username: a.tvheadendUsername, password: "" };
}

/** What a save would send, or why it cannot. Both login fields empty is a
 *  server anyone may watch; a username needs a password, typed or on file. */
export function planTvhSave(form: TvhForm, a: TvhOnFile): TvhSave | { error: string } {
  const url = form.url.trim();
  if (url === "") return { error: "a Tvheadend address is required" };
  const username = form.username.trim();
  if (username !== "" && form.password === "" && !a.tvheadendHasPassword) {
    return { error: "a password is required with a username (leave both empty if the server needs no login)" };
  }
  return { url, username, password: username === "" ? "" : form.password };
}

export function tvhChanged(form: TvhForm, a: TvhOnFile): boolean {
  return (
    form.url.trim() !== a.tvheadendUrl ||
    form.username.trim() !== a.tvheadendUsername ||
    form.password !== ""
  );
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
