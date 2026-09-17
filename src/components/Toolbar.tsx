// The window's own title bar, because there is no native one: Rust sets
// `decorations: false`, so everything a title bar does — dragging, the double
// click that maximises, minimise/maximise/close — is drawn and handled here.
//
// Three things share the bar, and the order is the whole design:
//
//   left    the hamburger, which holds every action the command palette holds,
//           because it is literally the palette's own list regrouped, and
//           beside it the one line saying what the backend is doing — sync had
//           a floating strip over the top-left of the window before this,
//           which is exactly what it was covering;
//   centre  the app's name, centred on the window rather than on the space
//           left over, so it does not drift as the status line grows;
//   right   the window buttons.
//
// Everything between the status and the window buttons drags the window, and
// the name is drawn over that strip without taking its clicks.
//
// Two rules that are not obvious:
//
//  1. Nothing interactive touches the window edge. The window is undecorated,
//     so Tauri's own resize handler owns the outer few pixels, and a button
//     flush against the top or the side would eat the grab. The bar keeps a
//     padding ring and every control lives inside it.
//  2. The bar is opaque chrome and is allowed to be, but it is never over the
//     video: everything below it is measured, and the rect handed to mpv comes
//     from that measurement. See `.stage` in styles.css.

import { useCallback, useEffect, useRef } from "react";
import { runAction } from "../actions";
import { useClock } from "../hooks/useClock";
import {
  ACTIONS,
  actionGroups,
  availableActions,
  type PaletteAction,
} from "../logic/palette";
import { maximiseLabel, statusLine } from "../logic/toolbar";
import { useApp, useDispatch } from "../store";
import { windowControls } from "../window";
import { useMaximised } from "../hooks/useMaximised";
import { Bolt } from "./icons";

export function Toolbar() {
  const state = useApp();
  const dispatch = useDispatch();
  const now = useClock(20_000);
  const maximised = useMaximised();
  const status = statusLine(state.sync, now);
  const menuRef = useRef<HTMLDivElement | null>(null);

  const closeMenu = useCallback(() => dispatch({ type: "menu", open: false }), [dispatch]);

  // Outside click. Escape is not handled here: it belongs to the one place
  // that decides what a single press unwinds — see useGlobalKeys.
  useEffect(() => {
    if (!state.menuOpen) return;
    const onDown = (e: MouseEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) closeMenu();
    };
    // Capture, so a click on a button somewhere else closes the menu before
    // that button acts rather than after it.
    window.addEventListener("mousedown", onDown, true);
    return () => window.removeEventListener("mousedown", onDown, true);
  }, [state.menuOpen, closeMenu]);

  const choose = (action: PaletteAction) => {
    closeMenu();
    runAction(action.id, { dispatch, player: state.player });
  };

  const max = maximiseLabel(maximised);

  return (
    <header className="toolbar">
      <div className="tb-lead">
        {/* The outside-click boundary is the button and its menu, and nothing
            else: clicking the status line beside them is outside. */}
        <div className="tb-menu-anchor" ref={menuRef}>
          <button
            className={`tb-btn tb-burger ${state.menuOpen ? "on" : ""}`}
            aria-label="Menu"
            aria-expanded={state.menuOpen}
            aria-haspopup="menu"
            title="Menu"
            onClick={() => dispatch({ type: "menu", open: !state.menuOpen })}
          >
            <Burger />
          </button>
          {state.menuOpen ? <Menu onChoose={choose} /> : null}
        </div>

        <Status
          {...status}
          onRetry={() => runAction("sync", { dispatch, player: state.player })}
        />
      </div>

      {/* Centred on the window, not on what is left between the status and the
          buttons, so it holds still while the status line changes length. It
          takes no clicks: the drag strip is underneath it. */}
      {/* The mark is the launcher's own path, so the two cannot drift. It sits
          inside the title, which takes no pointer events, so pressing it drags
          the window exactly as pressing the name does — a hole in the drag
          region here would be a dead spot in the middle of the title bar. */}
      <div className="tb-title" aria-hidden>
        <Bolt className="tb-bolt" />
        zapper
      </div>

      {/* The drag handle. `startDragging` hands the gesture to the window
          manager, which then swallows the mouse-up, so the double click has to
          be caught on the way down — `detail === 2` is the second press of the
          pair, before the drag would begin. */}
      <div
        className="tb-drag"
        data-drag-region
        onMouseDown={(e) => {
          if (e.button !== 0) return;
          if (e.detail >= 2) {
            void windowControls
              .toggleMaximise()
              .catch((err: unknown) =>
                dispatch({ type: "toast", text: `maximise: ${String(err)}` }),
              );
            return;
          }
          void windowControls.startDragging().catch(() => {
            // A window manager that refuses the drag is not worth a toast on
            // every press; the bar simply stays put.
          });
        }}
      />

      <div className="tb-tail">
        <button
          className="tb-btn"
          aria-label="Minimise"
          title="Minimise"
          onClick={() => void windowControls.minimise().catch(() => {})}
        >
          <span className="tb-glyph">—</span>
        </button>
        <button
          className="tb-btn"
          aria-label={max.label}
          title={max.label}
          onClick={() => void windowControls.toggleMaximise().catch(() => {})}
        >
          <span className="tb-glyph">{max.glyph}</span>
        </button>
        <button
          className="tb-btn danger"
          aria-label="Close"
          title="Close"
          onClick={() => void windowControls.close().catch(() => {})}
        >
          <span className="tb-glyph">✕</span>
        </button>
      </div>
    </header>
  );
}

/** Three rules, drawn rather than typed: a glyph would inherit the font's own
 *  idea of how thick "☰" is, and it differs wildly between systems. */
function Burger() {
  return (
    <span className="burger" aria-hidden>
      <i />
      <i />
      <i />
    </span>
  );
}

function Status({
  tone,
  text,
  detail,
  spinning,
  onRetry,
}: ReturnType<typeof statusLine> & { onRetry: () => void }) {
  return (
    <div className={`tb-status ${tone}`} role="status" aria-live="polite">
      {spinning ? <span className="tb-spinner" aria-hidden /> : null}
      <span className="tb-status-text">{text}</span>
      {detail ? <span className="tb-status-detail">{detail}</span> : null}
      {tone === "bad" ? (
        <button
          className="link"
          // The bar is a drag surface; a click on this button must not also
          // start dragging the window.
          onMouseDown={(e) => e.stopPropagation()}
          onClick={onRetry}
        >
          retry
        </button>
      ) : null}
    </div>
  );
}

function Menu({ onChoose }: { onChoose: (a: PaletteAction) => void }) {
  const { player } = useApp();
  const firstRef = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    firstRef.current?.focus();
  }, []);

  // Exactly the palette's list, regrouped. Anything the palette can do, the
  // menu can do, and nothing else — there is no second list to keep in step.
  const groups = actionGroups(availableActions(player.state !== "stopped"));
  let seen = 0;

  return (
    <div className="tb-menu" role="menu" aria-label="Main menu">
      {groups.map((group, gi) => (
        <div className="tb-menu-group" key={gi}>
          {group.map((a) => {
            const index = seen++;
            return (
              <button
                key={a.id}
                ref={index === 0 ? firstRef : undefined}
                className="tb-menu-item"
                role="menuitem"
                onClick={() => onChoose(a)}
              >
                <span className="tb-menu-label">{a.label}</span>
                <span className="tb-menu-hint">{a.hint ?? ""}</span>
              </button>
            );
          })}
        </div>
      ))}
      <div className="tb-menu-foot">
        {ACTIONS.length} actions · <b>Ctrl+P</b> searches them
      </div>
    </div>
  );
}
