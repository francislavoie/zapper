// One reducer holds everything the panes disagree about: which category is
// selected, which channel, what the search box says, whether the palette is
// open, and the two pieces of state the backend pushes at us (sync + player).
//
// No state library. A reducer plus a context is the whole thing.

import {
  createContext,
  useContext,
  useMemo,
  useReducer,
  type Dispatch,
  type ReactNode,
} from "react";
import type {
  CategoryDto,
  ChannelDto,
  PlayerStateDto,
  SettingsDto,
  SyncStatusDto,
  UpdateStatusDto,
} from "./types";
import { errorMessage, idle, type Async } from "./logic/async";
import { DEFAULT_SETTINGS, clampSettings } from "./logic/settings";
import { clampZoom, nextZoom, type ZoomCommand } from "./logic/zoom";
import { loadZoom } from "./zoom";
import { loadStacked } from "./stacked";
import { loadSession } from "./session";
import { loadVideoWidth } from "./videoSize";
import { loadRailWidth } from "./railWidth";
import { clampRailWidth } from "./logic/railWidth";
import { clampVideoWidth } from "./logic/videoSize";

export const ALL_CATEGORIES = "*all*";

/** The two halves of the rail. Categories is what the app opens on;
 *  favourites is the list nobody edits — it is earned from what gets
 *  watched, so there is no "add to favourites" anywhere. */
export type RailTab = "categories" | "favourites";

/** An open context menu over one favourite, and where the pointer was.
 *
 *  In the store rather than inside the component because Escape has to unwind
 *  it in the same single-press order as everything else, and that ordering
 *  lives in one place — see useGlobalKeys. */
export interface FavouriteMenu {
  streamId: number;
  name: string;
  x: number;
  y: number;
}

/** Browse shows the rail and the list with the picture in the watch panel;
 *  watch gives the whole window to video and only paints chrome; guide is the
 *  grid, which covers the window while the stream keeps playing underneath. */
export type Mode = "browse" | "watch" | "guide";

export interface Toast {
  id: number;
  text: string;
  kind: "error" | "info";
}

export interface State {
  mode: Mode;
  categories: Async<CategoryDto[]>;
  /** ALL_CATEGORIES for the pseudo-entry, else a CategoryDto.id. */
  categoryId: string;
  categoryFilter: string;
  /** Which half of the rail is showing. */
  railTab: RailTab;
  favouriteMenu: FavouriteMenu | null;
  /** Where a right-click on the picture landed, in client pixels. */
  videoMenu: { x: number; y: number } | null;
  /** The diagnostics panel. Off by default and polled only while open. */
  statsOpen: boolean;
  /** Which view fullscreen was entered from, so leaving it goes back there
   *  rather than stranding the viewer in the player. Null when fullscreen
   *  was entered from the player itself, where staying is correct. */
  fullscreenFrom: Mode | null;
  /** The text in the channel-search box. Empty means "browse the category".
   *  The results themselves are not stored: they belong to whichever pane
   *  asked for them, so the list and the palette cannot fight over one slot. */
  query: string;
  paletteOpen: boolean;
  settingsOpen: boolean;
  /** The toolbar's hamburger. Here rather than inside the component because
   *  Escape has to unwind it in the same single-press order as everything
   *  else, and that ordering lives in one place. */
  menuOpen: boolean;
  aboutOpen: boolean;
  /** The updater, as the backend last pushed it. Here rather than in the
   *  dialog because the toolbar's badge and the settings pane read it too. */
  update: UpdateStatusDto;
  updateOpen: boolean;
  sync: SyncStatusDto;
  player: PlayerStateDto;
  /** Optimistic: set the instant a row is clicked so the row highlights before
   *  the backend has opened anything. */
  pendingStreamId: number | null;
  /** Metadata for whatever is playing, painted immediately from SQLite while
   *  the stream opens. */
  nowChannel: ChannelDto | null;
  /** How far the guide has been paged away from now, in hours. */
  guideOffsetHours: number;
  /** Always a usable value: the contract's documented defaults until the
   *  backend answers. The load state is kept beside it rather than wrapping
   *  it in an Async, because the guide has to draw something either way. */
  settings: SettingsDto;
  settingsStatus: Async<null>;
  settingsSaveError: string | null;
  /** How big the interface is drawn, 1 being the design size. Not part of
   *  SettingsDto on purpose: that contract is Rust-side and synced, and how
   *  large this screen wants its type is a property of this machine. It is
   *  here rather than inside a component because the video rect depends on it
   *  — every layout box moves when it changes, and mpv has to be told. */
  zoom: number;
  /** How wide the docked picture is, in rem. Local like zoom, and here
   *  rather than in a component because resizing it moves the slot the video
   *  rect is measured from, so mpv has to be told. */
  videoWidthRem: number;
  /** The widest picture the stage has room for, in rem; `Infinity` until
   *  measured. What is drawn is the chosen width capped by this — see
   *  `fittedVideoWidth` — so a smaller window never overwrites the choice. */
  videoRoomRem: number;
  /** How wide the category/favourites rail is, in rem. Local like zoom. */
  railWidthRem: number;
  /** Whether watching should put the picture across the top with the guide
   *  underneath, rather than giving the whole window to video.
   *
   *  A preference, not a mode: it only takes effect while watching, and only
   *  when the window has the room for it (see logic/stack.ts). A window that
   *  is too small falls back to the ordinary player and comes back to this
   *  layout when it grows again, which is why the flag survives rather than
   *  being cleared. Remembered per machine, like zoom, for the same reason:
   *  a portrait monitor and a laptop want different answers. */
  stacked: boolean;
  toasts: Toast[];
}

export const initialSync: SyncStatusDto = {
  phase: "idle",
  message: "",
  channelsTotal: 0,
  lastSynced: null,
};

export const initialPlayer: PlayerStateDto = {
  state: "stopped",
  streamId: null,
  width: null,
  height: null,
  fps: null,
  codec: null,
  bufferedBack: 0,
  behindLive: 0,
  atLiveEdge: true,
  volume: 100,
  muted: false,
  compressor: false,
  error: null,
};

/** Until the backend answers: nothing known, nothing to show. */
export const initialUpdate: UpdateStatusDto = {
  current: "",
  kind: "none",
  lastChecked: null,
  state: { state: "idle" },
};

export const initialState: State = {
  mode: "browse",
  categories: idle(),
  // Where you were last time. Read before the first render, so the rail and
  // the list draw the right thing once rather than flicking from "All" to the
  // remembered category after a round trip.
  categoryId: loadSession().categoryId ?? ALL_CATEGORIES,
  categoryFilter: "",
  railTab: loadSession().railTab,
  favouriteMenu: null,
  videoMenu: null,
  statsOpen: false,
  fullscreenFrom: null,
  query: "",
  paletteOpen: false,
  settingsOpen: false,
  menuOpen: false,
  aboutOpen: false,
  update: initialUpdate,
  updateOpen: false,
  sync: initialSync,
  player: initialPlayer,
  pendingStreamId: null,
  nowChannel: null,
  guideOffsetHours: 0,
  settings: DEFAULT_SETTINGS,
  settingsStatus: idle(),
  settingsSaveError: null,
  // Read before the first render so the app opens at the size it was left at,
  // rather than at 100% for a frame.
  zoom: loadZoom(),
  videoWidthRem: loadVideoWidth(),
  videoRoomRem: Infinity,
  railWidthRem: loadRailWidth(),
  stacked: loadStacked(),
  toasts: [],
};

export type Action =
  | { type: "categories"; value: Async<CategoryDto[]> }
  | { type: "selectCategory"; id: string }
  | { type: "categoryFilter"; text: string }
  | { type: "railTab"; tab: RailTab }
  | { type: "favouriteMenu"; menu: FavouriteMenu | null }
  | { type: "videoMenu"; at: { x: number; y: number } | null }
  | { type: "stats"; open: boolean }
  | { type: "fullscreenFrom"; mode: Mode | null }
  | { type: "stacked"; on: boolean }
  | { type: "query"; text: string }
  | { type: "palette"; open: boolean }
  | { type: "settings"; open: boolean }
  | { type: "menu"; open: boolean }
  | { type: "about"; open: boolean }
  | { type: "update"; value: UpdateStatusDto }
  | { type: "updateDialog"; open: boolean }
  | { type: "mode"; mode: Mode }
  | { type: "sync"; value: SyncStatusDto }
  | { type: "player"; value: PlayerStateDto }
  | { type: "opening"; channel: ChannelDto }
  /** The channel from last time, restored but deliberately not started. */
  | { type: "ready"; channel: ChannelDto }
  | { type: "guideShift"; hours: number }
  | { type: "guideNow" }
  | { type: "settingsStatus"; value: Async<null> }
  | { type: "settingsValue"; value: SettingsDto }
  | { type: "settingsSaveError"; message: string | null }
  | { type: "zoom"; value: number }
  | { type: "zoomStep"; command: ZoomCommand }
  /** A width somebody chose. Capped by the room there is, because a drag
   *  can only show what fits; `reset` is the exception, since going back to
   *  the default is a choice about any window, not this one. */
  | { type: "videoWidth"; rem: number; reset?: boolean }
  | { type: "videoRoom"; rem: number }
  | { type: "railWidth"; rem: number }
  | { type: "toast"; text: string; kind?: "error" | "info" }
  | { type: "dismissToast"; id: number };

let toastSeq = 0;

export function reducer(state: State, action: Action): State {
  switch (action.type) {
    case "categories":
      return { ...state, categories: action.value };
    case "selectCategory":
      // Choosing a category is also a way out of a search: the two are one
      // list, and leaving the query up would make the choice look ignored.
      return { ...state, categoryId: action.id, query: "" };
    case "categoryFilter":
      return { ...state, categoryFilter: action.text };
    case "fullscreenFrom":
      return { ...state, fullscreenFrom: action.mode };
    case "videoMenu":
      return { ...state, videoMenu: action.at };
    case "stats":
      // Opening it closes the menu it was chosen from; nothing wants both.
      return { ...state, statsOpen: action.open, videoMenu: null };
    case "railTab":
      // A menu belongs to the list it was opened over; changing the list
      // leaves it pointing at a row that is no longer there.
      return { ...state, railTab: action.tab, favouriteMenu: null };
    case "favouriteMenu":
      return { ...state, favouriteMenu: action.menu };
    case "stacked":
      // Written to local storage by App, the same way zoom is: a reducer that
      // touched storage would do it twice under StrictMode and could not be
      // tested without one.
      return { ...state, stacked: action.on };
    case "query":
      return { ...state, query: action.text };
    case "palette":
      return { ...state, paletteOpen: action.open };
    case "settings":
      return {
        ...state,
        settingsOpen: action.open,
        menuOpen: action.open ? false : state.menuOpen,
      };
    case "menu":
      return { ...state, menuOpen: action.open };
    case "about":
      // Opening a dialog from the menu closes the menu: leaving it hanging
      // over its own result is how you end up with two things to dismiss.
      return { ...state, aboutOpen: action.open, menuOpen: false };
    case "update":
      return { ...state, update: action.value };
    case "updateDialog":
      // Same rule as About: chosen from the menu, so the menu goes.
      return { ...state, updateOpen: action.open, menuOpen: false };
    case "mode":
      // Any view change closes the menu, whoever asked for it — the menu is
      // one of the two places a view change comes from.
      return { ...state, mode: action.mode, menuOpen: false, favouriteMenu: null };
    case "sync":
      return { ...state, sync: action.value };
    case "player":
      return {
        ...state,
        player: action.value,
        // Once the backend confirms the stream we asked for, the optimistic
        // highlight has served its purpose.
        pendingStreamId:
          action.value.streamId === state.pendingStreamId
            ? null
            : state.pendingStreamId,
      };
    case "guideShift":
      return { ...state, guideOffsetHours: state.guideOffsetHours + action.hours };
    case "guideNow":
      return { ...state, guideOffsetHours: 0 };
    case "settingsStatus":
      return { ...state, settingsStatus: action.value };
    case "settingsValue":
      // Clamped on the way in as well as on the way out: a config the backend
      // hands back is not automatically sane.
      return { ...state, settings: clampSettings(action.value) };
    case "settingsSaveError":
      return { ...state, settingsSaveError: action.message };
    case "zoom":
      return { ...state, zoom: clampZoom(action.value) };
    case "zoomStep":
      // The step is computed here rather than by the caller, so that two
      // keystrokes in one tick are two stops. A caller reading the current
      // zoom and dispatching the result would read the same stale value twice
      // and move once — React has not re-rendered in between.
      return { ...state, zoom: nextZoom(state.zoom, action.command) };
    case "ready":
      // Only while nothing has claimed the band. Restoring is a round trip
      // through the catalogue and the user can start something while it is
      // in flight; replacing their choice with a memory of the last session
      // would be worse than not restoring at all.
      if (state.nowChannel !== null || state.player.state !== "stopped") return state;
      // Metadata only. No pendingStreamId, because nothing is opening, and no
      // mode change, because restoring where you were must not decide what
      // you are doing now.
      return { ...state, nowChannel: action.channel };
    case "videoWidth":
      return {
        ...state,
        videoWidthRem: clampVideoWidth(
          action.reset ? action.rem : Math.min(action.rem, state.videoRoomRem),
        ),
      };
    case "videoRoom":
      return action.rem === state.videoRoomRem ? state : { ...state, videoRoomRem: action.rem };
    case "railWidth":
      return { ...state, railWidthRem: clampRailWidth(action.rem) };
    case "opening":
      // Deliberately no mode change. Starting a channel is not a decision to
      // stop looking at whatever you were looking at, and the docked picture
      // exists precisely so it does not have to be. Callers that do mean
      // "and take over the window" say so.
      return {
        ...state,
        pendingStreamId: action.channel.streamId,
        nowChannel: action.channel,
      };
    case "toast":
      return {
        ...state,
        toasts: [
          ...state.toasts.slice(-2),
          { id: ++toastSeq, text: action.text, kind: action.kind ?? "error" },
        ],
      };
    case "dismissToast":
      return {
        ...state,
        toasts: state.toasts.filter((t) => t.id !== action.id),
      };
  }
}

const StateCtx = createContext<State>(initialState);
const DispatchCtx = createContext<Dispatch<Action>>(() => {});

export function StoreProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, initialState);
  const d = useMemo(() => dispatch, [dispatch]);
  return (
    <StateCtx.Provider value={state}>
      <DispatchCtx.Provider value={d}>{children}</DispatchCtx.Provider>
    </StateCtx.Provider>
  );
}

export const useApp = () => useContext(StateCtx);
export const useDispatch = () => useContext(DispatchCtx);

/** Run a backend command, and turn a rejection into a toast instead of an
 *  unhandled promise. The backend is not implemented yet: every one of these
 *  currently rejects with "not implemented", and the UI has to stay usable
 *  while that is true. */
export async function run(
  dispatch: Dispatch<Action>,
  what: string,
  p: Promise<unknown>,
): Promise<boolean> {
  try {
    await p;
    return true;
  } catch (e) {
    dispatch({ type: "toast", text: `${what}: ${errorMessage(e)}` });
    return false;
  }
}
