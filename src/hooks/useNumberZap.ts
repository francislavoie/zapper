// Typing a channel number to change channel, as on a TV remote. The rules of
// when an entry is finished are in logic/zap.ts; this is the keyboard, the
// clock and the catalogue around them.
//
// It listens in the capture phase, ahead of everything else, but only takes
// a key that is part of a number — or, while a number is being typed, Enter,
// Backspace and Escape. Enter has to be taken before it reaches whatever has
// focus: a focused row or button would otherwise act on it as well, and the
// viewer pressed it to say "that number", not to click anything. Escape the
// same: an entry in progress is the most recent thing on screen, so it is
// the one thing the press unwinds.
//
// The entry itself lives in this closure rather than in the store, because
// the decision on each key must see the keys before it — two digits in one
// tick are two digits, which a value read back from a render would not be.
// What is drawn goes to the store, where the picture's corner and the
// floating chip both read it.

import { useEffect, useRef, type Dispatch } from "react";
import { backend } from "../backend";
import { playChannel } from "../actions";
import { typingInto } from "../focus";
import { errorMessage } from "../logic/async";
import {
  ZAP_IDLE_MS,
  ZAP_MISS_MS,
  ZAP_TUNED_MS,
  buildNumberIndex,
  firstDecision,
  resolve,
  settle,
  typeChar,
  zapCells,
  zapChar,
  type NumberIndex,
} from "../logic/zap";
import type { Action, State } from "../store";
import type { ChannelDto } from "../types";

/** The catalogue's numbers, fetched on the first digit and kept until the
 *  next sync changes what the catalogue holds. Module-level so the list
 *  survives a remount; it is the same catalogue whoever asks. */
let cached: {
  synced: number | null;
  index: Promise<NumberIndex>;
  /** The same index once it has arrived, so a keystroke can be answered in
   *  the same frame rather than a promise later. */
  ready: NumberIndex | null;
} | null = null;
const channels = new Map<number, ChannelDto>();

function numberIndex(synced: number | null): Promise<NumberIndex> {
  if (cached === null || cached.synced !== synced) {
    channels.clear();
    const index = backend.channelNumbers().then(buildNumberIndex);
    const entry = { synced, index, ready: null as NumberIndex | null };
    index.then(
      (ready) => {
        entry.ready = ready;
      },
      () => {
        // A failed fetch is not kept: the next digit asks again.
        if (cached === entry) cached = null;
      },
    );
    cached = entry;
  }
  return cached.index;
}

function readyIndex(synced: number | null): NumberIndex | null {
  return cached !== null && cached.synced === synced ? cached.ready : null;
}

async function channel(streamId: number): Promise<ChannelDto | null> {
  const hit = channels.get(streamId);
  if (hit) return hit;
  const [row] = await backend.channelsByIds([streamId]);
  if (row) channels.set(streamId, row);
  return row ?? null;
}

/** Anything open on top of the views has the keyboard: a dialog, the
 *  palette, a menu. Digits there are not channel numbers. */
function covered(s: State): boolean {
  return (
    s.paletteOpen ||
    s.settingsOpen ||
    s.aboutOpen ||
    s.updateOpen ||
    s.menuOpen ||
    s.favouriteMenu !== null ||
    s.categoryMenu !== null ||
    s.categoryOptions !== null ||
    s.videoMenu !== null
  );
}

export function useNumberZap(state: State, dispatch: Dispatch<Action>) {
  const latest = useRef(state);
  latest.current = state;

  useEffect(() => {
    /** What has been typed so far; empty when no entry is open. */
    let typed = "";
    /** Bumped whenever the entry ends or changes, so an answer that arrives
     *  for an older state of it is dropped rather than drawn. */
    let gen = 0;
    /** Bumped per channel chosen. Separate from `gen`: a number that has been
     *  chosen is tuned even if the next one is already being typed. */
    let tuneSeq = 0;
    let idleTimer: number | undefined;
    let lingerTimer: number | undefined;

    const clearTimers = () => {
      window.clearTimeout(idleTimer);
      window.clearTimeout(lingerTimer);
      idleTimer = lingerTimer = undefined;
    };

    const linger = (ms: number) => {
      window.clearTimeout(lingerTimer);
      lingerTimer = window.setTimeout(() => dispatch({ type: "zap", value: null }), ms);
    };

    /** The entry is over, whatever comes of it. */
    const end = () => {
      typed = "";
      gen++;
      clearTimers();
    };

    const cancel = () => {
      end();
      dispatch({ type: "zap", value: null });
    };

    const miss = (label: string) => {
      dispatch({ type: "zap", value: { status: "miss", cells: [...label], label, name: null } });
      linger(ZAP_MISS_MS);
    };

    const tune = async (streamId: number, label: string) => {
      const mine = ++tuneSeq;
      let ch: ChannelDto | null;
      try {
        ch = await channel(streamId);
      } catch (e) {
        if (mine === tuneSeq) {
          dispatch({ type: "toast", text: `channel ${label}: ${errorMessage(e)}` });
        }
        return;
      }
      if (mine !== tuneSeq) return;
      // What it tuned is said only if no new number has been started in the
      // meantime; that one has the overlay now.
      const quiet = typed === "";
      if (!ch) {
        // Gone since the numbers were read: a sync dropped it.
        if (quiet) miss(label);
        return;
      }
      if (quiet) {
        dispatch({
          type: "zap",
          value: { status: "tuned", cells: [...label], label, name: ch.cleanName },
        });
        linger(ZAP_TUNED_MS);
      }
      // Already on: a remote leaves it alone rather than retuning, and
      // reopening a live stream costs the connect burst all over again.
      const p = latest.current.player;
      if (p.streamId === streamId && p.state !== "stopped" && p.state !== "failed") return;
      void playChannel(dispatch, ch);
    };

    /** Redraws the entry and acts if it is finished. */
    const update = async () => {
      const mine = ++gen;
      window.clearTimeout(lingerTimer);
      window.clearTimeout(idleTimer);
      idleTimer = window.setTimeout(() => void finish(), ZAP_IDLE_MS);

      const synced = latest.current.sync.lastSynced;
      let index = readyIndex(synced);
      if (index === null) {
        // The first digit of a session, before the numbers have arrived. It
        // is drawn at once, so the key visibly did something; the blanks
        // follow once it is known how many there can be.
        dispatch({
          type: "zap",
          value: { status: "typing", cells: [...typed], label: typed, name: null },
        });
        try {
          index = await numberIndex(synced);
        } catch (e) {
          if (mine !== gen) return;
          cancel();
          dispatch({ type: "toast", text: `channel numbers: ${errorMessage(e)}` });
          return;
        }
        if (mine !== gen) return;
      }

      // Read a key at a time: normally only the last key is new, but keys
      // typed while the numbers were on their way are all new at once.
      const { decision, rest } = firstDecision(index, typed);
      if (decision.kind !== "wait") {
        end();
        if (decision.kind === "tune") void tune(decision.streamId, decision.label);
        else miss(decision.label);
        if (rest !== "") {
          typed = rest;
          void update();
        }
        return;
      }

      const r = resolve(index, typed);
      const draw = (name: string | null) =>
        dispatch({
          type: "zap",
          value: { status: "typing", cells: zapCells(typed, r.slots), label: r.label, name },
        });
      if (r.exact === null) return draw(null);
      // The channel the number names as it stands — what Enter would tune.
      const known = channels.get(r.exact);
      draw(known?.cleanName ?? null);
      if (known) return;
      try {
        const ch = await channel(r.exact);
        if (mine === gen) draw(ch?.cleanName ?? null);
      } catch {
        // Only the name is missing; the number still tunes.
      }
    };

    /** Enter, or the pause: take the number as it stands. */
    const finish = async () => {
      if (typed === "") return;
      const mine = ++gen;
      window.clearTimeout(idleTimer);
      let index: NumberIndex;
      try {
        index = await numberIndex(latest.current.sync.lastSynced);
      } catch {
        // update() has said why, on the same failed fetch.
        if (mine === gen) cancel();
        return;
      }
      if (mine !== gen) return;
      const d = settle(index, typed);
      end();
      if (d.kind === "tune") void tune(d.streamId, d.label);
      else if (d.kind === "miss") miss(d.label);
    };

    const take = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopImmediatePropagation();
    };

    const onKey = (e: KeyboardEvent) => {
      const s = latest.current;
      if (typed !== "") {
        // Somewhere else took the keyboard mid-entry: let it have it.
        if (typingInto(e.target) || covered(s)) {
          cancel();
          return;
        }
        if (e.key === "Escape") {
          take(e);
          cancel();
          return;
        }
        if (e.key === "Enter" || e.key === "NumpadEnter") {
          take(e);
          void finish();
          return;
        }
        if (e.key === "Backspace") {
          take(e);
          typed = typed.slice(0, -1);
          if (typed === "") cancel();
          else void update();
          return;
        }
      }

      const ch = zapChar(e);
      if (ch === null) return;
      if (typingInto(e.target) || covered(s)) return;
      // A point cannot start a number; it is not taken, so it reaches
      // whatever else might want it.
      if (typed === "" && ch === ".") return;
      take(e);
      const next = typeChar(typed, ch);
      if (next === typed) return;
      typed = next;
      void update();
    };

    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      clearTimers();
      gen++;
    };
  }, [dispatch]);
}
