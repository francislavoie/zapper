// What the interface can do where it is running, as one object rather than
// checks for "in Tauri" scattered through the components.
//
// The same interface runs in three places: the app's own window (everything),
// the browser mock under `vite dev` (stands in for the app, so everything),
// and a browser signed in to the recording service (src/web.ts), which plays
// through the service's own stream in a <video> element (src/webPlayer.ts)
// and where whatever needs this computer's window, mpv or its files has no
// meaning. A component asks whether a capability is there; which backend
// grants it is decided once, in src/backend.ts.
//
// Checked in src/__checks__/capabilities.test.mjs.

export interface Capabilities {
  /** Plays channels here: the picture, the transport, audio tracks, the
   *  sleep timer, recall. */
  playback: boolean;
  /** The provider's archive: a finished programme from the guide, and
   *  seeking past what the player holds. */
  catchup: boolean;
  /** mpv's own: Settings › Playback, Audio and Subtitles, the account's
   *  connection count, subtitles, deinterlacing, and stats for nerds. */
  mpv: boolean;
  /** The undecorated window's own title bar: dragging, minimise, maximise,
   *  close, the resize edges, Quit. */
  window: boolean;
  /** The desktop's media keys and controls. */
  mediaKeys: boolean;
  /** Looks for, downloads and installs new versions of the app. */
  updater: boolean;
  /** Changes where the channels come from: the provider account and
   *  testing it (Settings › Account). Its connections and how many streams
   *  it allows are about playing here, and go with `playback`. */
  account: boolean;
  /** Refreshes the catalogue from the provider now: Settings › Data's
   *  Refresh, the toolbar's retry. */
  resync: boolean;
  /** Backups through the system's file dialogs (Settings › Data), and how
   *  often the catalogue refreshes. */
  data: boolean;
  /** Installs and removes the recording service on this computer, and opens
   *  a recording's folder in its file manager. */
  localService: boolean;
  /** The recording service's network access, sign-in password and devices:
   *  the app's alone (docs/recording.md). */
  networkAdmin: boolean;
  /** Signed in to the recording service with its password, and so able to
   *  sign out. */
  signOut: boolean;
  /** Movies from the provider's on-demand catalogue. Whether there are any to
 *  show is the account's, and the movies summary says. */
  onDemand: boolean;
}

/** The app's window, and the mock that stands in for it. */
export const DESKTOP: Capabilities = {
  playback: true,
  catchup: true,
  mpv: true,
  window: true,
  mediaKeys: true,
  updater: true,
  account: true,
  resync: true,
  data: true,
  localService: true,
  networkAdmin: true,
  signOut: false,
  onDemand: true,
};

/** A browser signed in to the recording service. Browsing, the guide,
 *  search and recordings, the provider account and refreshing from it (the
 *  service keeps both, docs/recording.md), and watching live through the
 *  service (where the browser has Media Source Extensions: src/backend.ts
 *  turns `playback` off where it hasn't); nothing of this computer's. */
export const BROWSER: Capabilities = {
  playback: true,
  catchup: false,
  mpv: false,
  window: false,
  mediaKeys: false,
  updater: false,
  account: true,
  resync: true,
  data: false,
  localService: false,
  networkAdmin: false,
  signOut: true,
  onDemand: true,
};

export type Capability = keyof Capabilities;

/** The pages of Settings, in order. */
export const SETTINGS_SECTIONS = [
  "Account",
  "Playback",
  "Audio",
  "Subtitles",
  "Guide",
  "Recording",
  "Data",
  "Updates",
  "Keyboard",
  "About",
] as const;

export type SettingsSection = (typeof SETTINGS_SECTIONS)[number];

/** What each page needs; a page not listed needs nothing. */
const SECTION_NEEDS: Partial<Record<SettingsSection, Capability>> = {
  Account: "account",
  Playback: "mpv",
  Audio: "mpv",
  Subtitles: "mpv",
  Data: "resync",
  Updates: "updater",
};

/** The pages of Settings there is something to do on here. */
export function settingsSections(caps: Capabilities): SettingsSection[] {
  return SETTINGS_SECTIONS.filter((s) => {
    const need = SECTION_NEEDS[s];
    return need === undefined || caps[need];
  });
}

/** Whether `need`, if any, is granted. */
export function allows(caps: Capabilities, need: Capability | undefined): boolean {
  return need === undefined || caps[need];
}
