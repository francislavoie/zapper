// Continue watching's slice of the store, with a reducer of its own as the
// Movies and Shows tabs have (see moviesState.ts): the list both tabs show at
// the top of their rails, the entry open on it, its menu, and the question a
// held open asks. The store delegates every `continue/*` action here.

import type { Async } from "./logic/async.ts";
import { idle } from "./logic/async.ts";
import { continueKey } from "./logic/continueWatching.ts";
import type { ResumePrompt } from "./logic/resume.ts";
import type { ContinueItemDto } from "./types";

/** An entry's menu and where it is drawn, in window pixels. `takeFocus`: the
 *  keyboard opened it, so focus moves into it. */
export interface ContinueMenuAt {
  key: string;
  x: number;
  y: number;
  takeFocus: boolean;
}

export interface ContinueState {
  items: Async<ContinueItemDto[]>;
  /** The entry the details pane shows. */
  selected: ContinueItemDto | null;
  menu: ContinueMenuAt | null;
  /** The question a held open asks, while it does. */
  prompt: ResumePrompt | null;
  /** Bumped when the list may have changed (a mark, a stop, a removal), so
   *  it is read again. */
  revision: number;
}

export function initialContinue(): ContinueState {
  return { items: idle(), selected: null, menu: null, prompt: null, revision: 0 };
}

export type ContinueAction =
  /** `revision`: the state's revision when the read began; an answer that
   *  finds it moved on (another account, a mark) is out of date and dropped. */
  | { type: "continue/items"; value: Async<ContinueItemDto[]>; revision?: number }
  | { type: "continue/select"; item: ContinueItemDto | null }
  | { type: "continue/menu"; menu: ContinueMenuAt | null }
  | { type: "continue/prompt"; prompt: ResumePrompt | null }
  | { type: "continue/changed" }
  /** Another account's titles: nothing kept of the old list applies. */
  | { type: "continue/reset" };

export function isContinueAction(a: { type: string }): a is ContinueAction {
  return a.type.startsWith("continue/");
}

export function continueReducer(s: ContinueState, a: ContinueAction): ContinueState {
  switch (a.type) {
    case "continue/items": {
      if (a.revision !== undefined && a.revision !== s.revision) return s;
      // A list already showing is neither blanked by the fetch that refreshes
      // it nor replaced by one that failed.
      if (a.value.status !== "ready" && s.items.status === "ready") return s;
      if (a.value.status !== "ready") return { ...s, items: a.value };
      const fresh = a.value.data;
      const was = s.selected ? continueKey(s.selected) : null;
      // An entry gone (removed, marked watched) hands the selection to the
      // one now in its place, clamped to the end: only the selection moves,
      // nothing plays.
      let selected: ContinueItemDto | null = null;
      if (was !== null) {
        selected = fresh.find((i) => continueKey(i) === was) ?? null;
        if (selected === null && fresh.length > 0) {
          const old = s.items.status === "ready" ? s.items.data.findIndex((i) => continueKey(i) === was) : -1;
          selected = fresh[Math.min(Math.max(old, 0), fresh.length - 1)];
        }
      }
      // A menu over an entry that has gone goes with it.
      const menu = s.menu && fresh.some((i) => continueKey(i) === s.menu?.key) ? s.menu : null;
      return { ...s, items: a.value, selected, menu };
    }
    case "continue/select":
      return { ...s, selected: a.item, menu: null };
    case "continue/menu":
      return { ...s, menu: a.menu };
    case "continue/prompt":
      return a.prompt === s.prompt ? s : { ...s, prompt: a.prompt };
    case "continue/changed":
      return { ...s, revision: s.revision + 1 };
    case "continue/reset":
      return { ...s, items: idle(), selected: null, menu: null, revision: s.revision + 1 };
  }
}
