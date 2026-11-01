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
  ChannelQualityDto,
  LiveJumpDto,
  PlayerOptionsDto,
  PlayerStateDto,
  ProgrammeDto,
  QualityDto,
  SettingsDto,
  SyncStatusDto,
  UpdateStatusDto,
} from "./types";
import { errorMessage, idle, type Async } from "./logic/async";
import { DEFAULT_SETTINGS, clampSettings } from "./logic/settings";
import { setTimeFormat } from "./logic/format";
import { HOUR, clampOffset, rebaseOffset } from "./logic/guide";
import { clampZoom, nextZoom, type ZoomCommand } from "./logic/zoom";
import { loadZoom } from "./zoom";
import { loadStacked } from "./stacked";
import { loadSession } from "./session";
import { loadVideoWidth } from "./videoSize";
import { loadRailWidth } from "./railWidth";
import { clampRailWidth } from "./logic/railWidth";
import { clampVideoWidth } from "./logic/videoSize";
import { setHidden, setPinned, type CategorySort } from "./logic/categoryOrder";
import { loadCategoryHidden, loadCategoryPins, loadCategorySort } from "./categoryPrefs";
import type { ZapView } from "./logic/zap";
import { startSleep, type SleepRequest, type SleepTimer } from "./logic/sleep";

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

/** An open context menu over one category row — pin or unpin it. Here for
 *  the same reason as FavouriteMenu: Escape unwinds it in useGlobalKeys.
 *
 *  `keyboard` when it was opened with the menu key or Shift+F10, so it takes
 *  focus — a menu that appears at a row the keyboard is on and cannot be
 *  reached from the keyboard is no menu at all — and hands it back on close. */
export interface CategoryMenu {
  id: string;
  name: string;
  x: number;
  y: number;
  keyboard: boolean;
}

/** The menu over the picture: where it opens, in client pixels, and what it
 *  holds. In the store for the same reason as the others — Escape unwinds it
 *  in useGlobalKeys.
 *
 *  `only` is the menu narrowed to one of its submenus, opened from a control
 *  rather than a right-click: the transport bar's audio button and its moon.
 *  `anchor` is that control, so a second press on it closes the menu rather
 *  than closing and reopening it; `keyboard` when it was pressed from the
 *  keyboard, so the menu takes focus. */
export interface VideoMenu {
  x: number;
  y: number;
  only?: "audio" | "sleep";
  anchor?: HTMLElement | null;
  keyboard?: boolean;
  /** Opens upwards from `y`: from a control at the bottom of the picture. */
  above?: boolean;
}

/** Browse shows the rail and the list with the picture in the watch panel;
 *  watch gives the whole window to video and only paints chrome; guide is the
 *  grid, which covers the window while the stream keeps playing underneath. */
export type Mode = "browse" | "watch" | "guide";

/** See `State.guideFocus`. */
export interface GuideFocus {
  streamId: number;
  index: number | null;
  seq: number;
}

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
  /** The rail folded away, in the list and the guide alike: one rail serves
   *  both, so one switch hides it from both. */
  railCollapsed: boolean;
  favouriteMenu: FavouriteMenu | null;
  categoryMenu: CategoryMenu | null;
  /** Category ids pinned to the top of the rail, in the order they were
   *  pinned. May name categories that no longer exist; see
   *  logic/categoryOrder.ts for why those are kept. Local like zoom. */
  categoryPins: readonly string[];
  /** How the unpinned categories are ordered. Local like zoom. */
  categorySort: CategorySort;
  /** Category ids taken out of the rail. Kept like pins — unknown ids too —
   *  and independent of them: a hidden pin keeps its place for when it is
   *  unhidden. Local like zoom. */
  categoryHidden: readonly string[];
  /** Draw the hidden categories anyway, dimmed, so they can be unhidden one
   *  by one. Not remembered: hidden is what the rail should be at launch. */
  categoryShowHidden: boolean;
  /** Bumped when watch history changes other than by watching — an import —
   *  so the favourites rail knows to ask again. */
  activityVersion: number;
  /** Channel records that playing has changed since their rows were
   *  fetched, by stream id: the list draws these over what its pages hold,
   *  so a channel that just failed dims at once and one that just played
   *  says what was measured. */
  qualityNews: Readonly<Record<number, QualityDto | null>>;
  /** The rail's options menu, under the ▾ beside the sort button: where it
   *  opens, in client pixels. In the store for the same reason as
   *  categoryMenu — Escape unwinds it in useGlobalKeys. */
  categoryOptions: { x: number; y: number } | null;
  /** The menu over the picture, if it is open. */
  videoMenu: VideoMenu | null;
  /** The stream's tracks and how the picture is drawn, as the backend last
   *  pushed them. */
  options: PlayerOptionsDto;
  /** The sleep timer, while one is set. Cleared when it goes off, when it is
   *  turned off, and whenever playback stops: an explicit stop is the
   *  viewer being awake, and with nothing playing there is nothing for it
   *  to stop. */
  sleep: SleepTimer | null;
  /** What the guide says is on the playing channel now, when a sleep timer
   *  may end with it — see `programmeForSleep`. Kept by the SleepTimer
   *  component, which watches the clock so nothing else has to. */
  sleepProgramme: ProgrammeDto | null;
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
  /** The channel before this one, for P: a remote's recall button. Set when
   *  a different channel opens, so recalling it makes the current one the
   *  previous and P flips between the two. Remembered across launches with
   *  the session, beside the channel itself. */
  previousChannel: ChannelDto | null;
  /** A channel number being typed, as drawn; null when none is. Driven by
   *  useNumberZap. */
  zap: ZapView | null;
  /** The automatic catch-up to live being announced over the picture, or
   *  null. Raised when the backend reports a new one, cleared a few seconds
   *  later by App. */
  liveJump: LiveJumpDto | null;
  /** How far the guide has been scrolled away from now, in seconds from
   *  `guideAnchor`; zero follows the clock. Seconds rather than a scrollLeft
   *  because the guide never has a wide strip to scroll — see "scrolling in
   *  time" in logic/guide.ts. Here rather than in the grid so that the keys,
   *  which live in useGlobalKeys, and a guide closed and reopened agree on
   *  where it was. */
  guideOffset: number;
  /** The half hour `guideOffset` is measured from, or null before the guide
   *  has drawn. Kept so the offset can be carried across the clock rolling
   *  over — the grid reports each new anchor with `guideRebase`. */
  guideAnchor: number | null;
  /** A channel the guide was sent to — by choosing an upcoming programme in
   *  search — to be scrolled into view and marked. `index` is its row in the
   *  category the guide shows, or null when that could not be found and the
   *  guide can only mark it if it happens to be on screen. `seq` makes a
   *  second trip to the same channel a new request. Cleared by choosing a
   *  category, which is somewhere else entirely. */
  guideFocus: GuideFocus | null;
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
  liveJump: null,
  ending: null,
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
  railCollapsed: loadSession().railCollapsed,
  favouriteMenu: null,
  categoryMenu: null,
  // Read before the first render, like the rail's width, so the rail draws in
  // its remembered order once rather than reshuffling a frame later.
  categoryPins: loadCategoryPins(),
  categorySort: loadCategorySort(),
  categoryHidden: loadCategoryHidden(),
  categoryShowHidden: false,
  activityVersion: 0,
  qualityNews: {},
  categoryOptions: null,
  videoMenu: null,
  options: { audio: [], subtitles: [], aspect: "auto", deinterlace: "off" },
  sleep: null,
  sleepProgramme: null,
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
  previousChannel: null,
  zap: null,
  liveJump: null,
  guideOffset: 0,
  guideAnchor: null,
  guideFocus: null,
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
  | { type: "railCollapsed"; collapsed: boolean }
  | { type: "favouriteMenu"; menu: FavouriteMenu | null }
  | { type: "categoryMenu"; menu: CategoryMenu | null }
  | { type: "categoryPin"; id: string; pinned: boolean }
  | { type: "categorySort"; sort: CategorySort }
  | { type: "categoryHide"; id: string; hidden: boolean }
  | { type: "categoryUnhideAll" }
  | { type: "categoryShowHidden"; on: boolean }
  /** All three of the rail's choices at once, from a backup. */
  | { type: "categoryPrefs"; pins: readonly string[]; hidden: readonly string[]; sort: CategorySort }
  | { type: "activityChanged" }
  | { type: "categoryOptions"; menu: { x: number; y: number } | null }
  | { type: "videoMenu"; at: VideoMenu | null }
  | { type: "options"; value: PlayerOptionsDto }
  /** Set the sleep timer, or turn it off with `null`. `at` is the wall clock
   *  in milliseconds, passed in so the reducer stays pure. */
  | { type: "sleep"; request: SleepRequest | null; at: number }
  | { type: "sleepProgramme"; programme: ProgrammeDto | null }
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
  | { type: "channelQuality"; value: ChannelQualityDto }
  | { type: "opening"; channel: ChannelDto }
  /** The channel from last time, restored but deliberately not started, and
   *  the one before it, for P. */
  | { type: "ready"; channel: ChannelDto | null; previous?: ChannelDto | null }
  | { type: "zap"; value: ZapView | null }
  /** The announcement of jump `seq` has been up long enough. */
  | { type: "liveJumpSeen"; seq: number }
  /** A step: ‹ › and the arrow keys by an hour, a click on the scrollbar's
   *  track by a screen. */
  | { type: "guideShift"; hours: number }
  /** A wheel or swipe, by however far it moved. */
  | { type: "guideScroll"; seconds: number }
  /** A place on the scrollbar: an offset from `anchor`, which the grid passes
   *  because it may be a half hour ahead of the store's. */
  | { type: "guideSeek"; offset: number; anchor: number }
  | { type: "guideRebase"; anchor: number }
  | { type: "guideNow" }
  /** Open the guide on `categoryId` at `offset` from `anchor`, with the
   *  channel at row `index` scrolled to and marked. */
  | {
      type: "guideFocus";
      categoryId: string;
      streamId: number;
      index: number | null;
      offset: number;
      anchor: number;
    }
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
      return { ...state, categoryId: action.id, query: "", guideFocus: null };
    case "guideFocus":
      // The guide, at a time, on a channel's row. The query is kept: the
      // search this came from is still there to go back to.
      return {
        ...state,
        mode: "guide",
        categoryId: action.categoryId,
        guideOffset: clampOffset(action.offset, state.settings.guideHours),
        guideAnchor: action.anchor,
        guideFocus: {
          streamId: action.streamId,
          index: action.index,
          seq: (state.guideFocus?.seq ?? 0) + 1,
        },
        menuOpen: false,
        favouriteMenu: null,
        categoryMenu: null,
        categoryOptions: null,
      };
    case "categoryFilter":
      return { ...state, categoryFilter: action.text };
    case "fullscreenFrom":
      return { ...state, fullscreenFrom: action.mode };
    case "videoMenu":
      return { ...state, videoMenu: action.at };
    case "options":
      return { ...state, options: action.value };
    case "sleep": {
      // Chosen from a menu, so the menu has done its job either way.
      const closed = { ...state, videoMenu: null, menuOpen: false };
      if (action.request === null) return { ...closed, sleep: null };
      // A request that cannot be had — the programme's end went unknown
      // between the menu drawing and the click — leaves the timer as it was.
      const timer = startSleep(action.request, action.at, state.sleepProgramme);
      return timer ? { ...closed, sleep: timer } : closed;
    }
    case "sleepProgramme":
      return state.sleepProgramme === action.programme
        ? state
        : { ...state, sleepProgramme: action.programme };
    case "stats":
      // Opening it closes the menu it was chosen from; nothing wants both.
      return { ...state, statsOpen: action.open, videoMenu: null };
    case "railTab":
      // A menu belongs to the list it was opened over; changing the list
      // leaves it pointing at a row that is no longer there.
      return {
        ...state,
        railTab: action.tab,
        favouriteMenu: null,
        categoryMenu: null,
        categoryOptions: null,
      };
    case "railCollapsed":
      // The menus hang off rows that are about to stop being on screen.
      return state.railCollapsed === action.collapsed
        ? state
        : {
            ...state,
            railCollapsed: action.collapsed,
            favouriteMenu: null,
            categoryMenu: null,
            categoryOptions: null,
          };
    case "favouriteMenu":
      return { ...state, favouriteMenu: action.menu };
    case "categoryMenu":
      return { ...state, categoryMenu: action.menu };
    case "categoryPin":
      // Written to local storage by App, like the rail's width. The menu it
      // may have been chosen from has done its job either way.
      return {
        ...state,
        categoryPins: setPinned(state.categoryPins, action.id, action.pinned),
        categoryMenu: null,
      };
    case "categorySort":
      return { ...state, categorySort: action.sort, categoryOptions: null };
    case "categoryHide":
      // Hiding the category being browsed leaves it selected: the list keeps
      // showing what it was showing, and only the rail loses the row. Jumping
      // to "All channels" would throw away the list and its scroll position
      // over what was a tidy-up of the rail, not a request to go anywhere.
      return {
        ...state,
        categoryHidden: setHidden(state.categoryHidden, action.id, action.hidden),
        categoryMenu: null,
      };
    case "categoryUnhideAll":
      return {
        ...state,
        categoryHidden: state.categoryHidden.length === 0 ? state.categoryHidden : [],
        categoryOptions: null,
      };
    case "categoryShowHidden":
      return { ...state, categoryShowHidden: action.on, categoryOptions: null };
    case "categoryPrefs":
      // Saved by App's effects, like any other change to the three.
      return {
        ...state,
        categoryPins: action.pins,
        categoryHidden: action.hidden,
        categorySort: action.sort,
      };
    case "activityChanged":
      return { ...state, activityVersion: state.activityVersion + 1 };
    case "categoryOptions":
      return { ...state, categoryOptions: action.menu };
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
      return {
        ...state,
        mode: action.mode,
        menuOpen: false,
        favouriteMenu: null,
        categoryMenu: null,
        categoryOptions: null,
      };
    case "sync":
      return { ...state, sync: action.value };
    case "channelQuality":
      return {
        ...state,
        qualityNews: { ...state.qualityNews, [action.value.streamId]: action.value.quality },
      };
    case "player":
      return {
        ...state,
        player: action.value,
        liveJump: nextLiveJump(state, action.value),
        // Stopping is the viewer awake, or the stream gone: either way there
        // is nothing left for the timer to do.
        sleep:
          action.value.state === "stopped" && state.player.state !== "stopped"
            ? null
            : state.sleep,
        // Once the backend confirms the stream we asked for, the optimistic
        // highlight has served its purpose.
        pendingStreamId:
          action.value.streamId === state.pendingStreamId
            ? null
            : state.pendingStreamId,
      };
    // Clamped here as well as where it is drawn, so that holding ← at the
    // far edge does not wind up an offset that then takes as many presses of
    // → to unwind before anything moves.
    case "guideShift":
    case "guideScroll": {
      const by = action.type === "guideShift" ? action.hours * HOUR : action.seconds;
      const offset = clampOffset(state.guideOffset + by, state.settings.guideHours);
      return offset === state.guideOffset ? state : { ...state, guideOffset: offset };
    }
    case "guideSeek": {
      const offset = clampOffset(action.offset, state.settings.guideHours);
      return offset === state.guideOffset && action.anchor === state.guideAnchor
        ? state
        : { ...state, guideOffset: offset, guideAnchor: action.anchor };
    }
    case "guideRebase":
      if (state.guideAnchor === action.anchor) return state;
      return {
        ...state,
        guideAnchor: action.anchor,
        guideOffset: rebaseOffset(
          state.guideOffset,
          state.guideAnchor,
          action.anchor,
          state.settings.guideHours,
        ),
      };
    case "guideNow":
      return state.guideOffset === 0 ? state : { ...state, guideOffset: 0 };
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
    case "ready": {
      // Only while nothing has claimed the band. Restoring is a round trip
      // through the catalogue and the user can start something while it is
      // in flight; replacing their choice with a memory of the last session
      // would be worse than not restoring at all. The same goes for P's
      // channel: one started in the meantime has already set it.
      if (state.nowChannel !== null || state.player.state !== "stopped") return state;
      // Metadata only. No pendingStreamId, because nothing is opening, and no
      // mode change, because restoring where you were must not decide what
      // you are doing now.
      const previous =
        action.previous && action.previous.streamId !== action.channel?.streamId
          ? action.previous
          : null;
      return {
        ...state,
        nowChannel: action.channel,
        previousChannel: state.previousChannel ?? previous,
      };
    }
    case "zap":
      return state.zap === action.value ? state : { ...state, zap: action.value };
    case "liveJumpSeen":
      return state.liveJump?.seq === action.seq ? { ...state, liveJump: null } : state;
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
        // Only a different channel moves the recall: a catch-up programme on
        // the channel already on, or the same channel reopened, leaves P
        // pointing where it was.
        previousChannel:
          state.nowChannel && state.nowChannel.streamId !== action.channel.streamId
            ? state.nowChannel
            : state.previousChannel,
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

/** The catch-up to announce after a player update: a new one when the
 *  backend reports a jump it had not reported before, nothing once the
 *  stream it happened on has gone, and otherwise whatever was showing. */
function nextLiveJump(state: State, next: PlayerStateDto): LiveJumpDto | null {
  const jump = next.liveJump;
  if (jump && jump.seq !== state.player.liveJump?.seq) return jump;
  if (next.streamId !== state.player.streamId || next.state === "stopped") return null;
  return state.liveJump;
}

const StateCtx = createContext<State>(initialState);
const DispatchCtx = createContext<Dispatch<Action>>(() => {});

export function StoreProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, initialState);
  const d = useMemo(() => dispatch, [dispatch]);
  // The time helpers' clock, set here as this renders: everything showing a
  // time renders after it, so nothing draws one in the old format first.
  setTimeFormat(state.settings.timeFormat);
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
