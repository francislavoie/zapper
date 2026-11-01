// What the interface can do where it is running, as one object rather than
// checks for "in Tauri" scattered through the components.
//
// The same interface runs in three places: the app's own window (everything),
// the browser mock under `vite dev` (stands in for the app, so everything),
// and a browser signed in to the recording service (src/web.ts), where
// whatever needs this computer's window, its player or its files has no
// meaning. A component asks whether a capability is there; which backend
// grants it is decided once, in src/backend.ts.
//
// Checked in src/__checks__/capabilities.test.mjs.

export interface Capabilities {
  /** Plays channels here: the picture, the transport, tracks, stats, the
   *  sleep timer, recall. */
  playback: boolean;
  /** The undecorated window's own title bar: dragging, minimise, maximise,
   *  close, the resize edges, Quit. */
  window: boolean;
  /** The desktop's media keys and controls. */
  mediaKeys: boolean;
  /** Looks for, downloads and installs new versions of the app. */
  updater: boolean;
  /** Changes where the channels come from: the provider account, testing it,
   *  its connections and how many streams it allows. */
  account: boolean;
  /** Settings › Data (backups through the system's file dialogs) and how
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
}

/** The app's window, and the mock that stands in for it. */
export const DESKTOP: Capabilities = {
  playback: true,
  window: true,
  mediaKeys: true,
  updater: true,
  account: true,
  data: true,
  localService: true,
  networkAdmin: true,
  signOut: false,
};

/** A browser signed in to the recording service. Browsing, the guide,
 *  search and recordings; nothing of this computer's. */
export const BROWSER: Capabilities = {
  playback: false,
  window: false,
  mediaKeys: false,
  updater: false,
  account: false,
  data: false,
  localService: false,
  networkAdmin: false,
  signOut: true,
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
  Playback: "playback",
  Audio: "playback",
  Subtitles: "playback",
  Data: "data",
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
