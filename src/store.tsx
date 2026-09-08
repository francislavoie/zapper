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
} from "./types";
import { errorMessage, idle, type Async } from "./logic/async";
import { DEFAULT_SETTINGS, clampSettings } from "./logic/settings";

export const ALL_CATEGORIES = "*all*";

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
  /** The text in the channel-search box. Empty means "browse the category".
   *  The results themselves are not stored: they belong to whichever pane
   *  asked for them, so the list and the palette cannot fight over one slot. */
  query: string;
  paletteOpen: boolean;
  settingsOpen: boolean;
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
  volume: 100,
  muted: false,
  compressor: false,
  error: null,
};

export const initialState: State = {
  mode: "browse",
  categories: idle(),
  categoryId: ALL_CATEGORIES,
  categoryFilter: "",
  query: "",
  paletteOpen: false,
  settingsOpen: false,
  sync: initialSync,
  player: initialPlayer,
  pendingStreamId: null,
  nowChannel: null,
  guideOffsetHours: 0,
  settings: DEFAULT_SETTINGS,
  settingsStatus: idle(),
  settingsSaveError: null,
  toasts: [],
};

export type Action =
  | { type: "categories"; value: Async<CategoryDto[]> }
  | { type: "selectCategory"; id: string }
  | { type: "categoryFilter"; text: string }
  | { type: "query"; text: string }
  | { type: "palette"; open: boolean }
  | { type: "settings"; open: boolean }
  | { type: "mode"; mode: Mode }
  | { type: "sync"; value: SyncStatusDto }
  | { type: "player"; value: PlayerStateDto }
  | { type: "opening"; channel: ChannelDto }
  | { type: "guideShift"; hours: number }
  | { type: "guideNow" }
  | { type: "settingsStatus"; value: Async<null> }
  | { type: "settingsValue"; value: SettingsDto }
  | { type: "settingsSaveError"; message: string | null }
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
    case "query":
      return { ...state, query: action.text };
    case "palette":
      return { ...state, paletteOpen: action.open };
    case "settings":
      return { ...state, settingsOpen: action.open };
    case "mode":
      return { ...state, mode: action.mode };
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
    case "opening":
      return {
        ...state,
        pendingStreamId: action.channel.streamId,
        nowChannel: action.channel,
        mode: "watch",
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
