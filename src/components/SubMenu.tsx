// A row in a menu that opens a second menu beside it: "Audio track ›".
//
// The menus in this app are short lists, and four lists of choices — tracks,
// subtitles, aspect, deinterlacing — laid end to end would make the picture's
// menu taller than the picture. Each becomes one row saying what is chosen
// now, and the choices open beside it on hover, on a click, or on → / Enter.
//
// The panel is drawn inside the parent menu's own element, not in a portal,
// so everything that asks "is this inside the menu?" — the outside-click
// dismissal, the parent's arrow keys — gets the right answer for free. It is
// positioned with `position: fixed` and then corrected by measurement, which
// also holds when an ancestor (a backdrop filter, say) makes a containing
// block of its own.
//
// Only one submenu of a menu is open at a time. `SubMenuHost` is what knows
// which: it wraps a menu's items, and the pointer arriving on any other item
// of that menu closes the open one.

import {
  createContext,
  useContext,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { documentScale } from "../zoom";
import { Check, ChevronRight } from "./icons";
import { subMenuPosition } from "../logic/favourites";

const ITEM = '[role="menuitem"], [role="menuitemcheckbox"], [role="menuitemradio"]';

interface Host {
  open: string | null;
  setOpen: (id: string | null) => void;
}

const HostCtx = createContext<Host | null>(null);

/** Wraps one menu's items. Renders no box of its own (`display: contents`),
 *  so it can sit inside whatever layout the menu already has. */
export function SubMenuHost({ children }: { children: ReactNode }) {
  const [open, setOpen] = useState<string | null>(null);
  return (
    <HostCtx.Provider value={{ open, setOpen }}>
      <div
        className="submenu-host"
        onPointerOver={(e) => {
          if (open === null) return;
          const target = e.target as HTMLElement;
          // Moving within the open panel, or onto its own row, keeps it.
          if (target.closest(".ctx-sub")) return;
          // Another submenu's row opens its own on entry, which replaces this
          // one; closing here as well would race it, since React may deliver
          // the row's enter before this over.
          const item = target.closest<HTMLElement>(ITEM);
          if (item && item.dataset.submenu === undefined) setOpen(null);
        }}
      >
        {children}
      </div>
    </HostCtx.Provider>
  );
}

export interface SubMenuProps {
  /** What the row is: "Audio track". Also the panel's accessible name. */
  label: string;
  /** What is chosen now, said quietly on the row: "English". */
  value?: string;
  children: ReactNode;
}

export function SubMenu({ label, value, children }: SubMenuProps) {
  const host = useContext(HostCtx);
  const id = useId();
  // Outside a host it still works, alone.
  const [ownOpen, setOwnOpen] = useState(false);
  const open = host ? host.open === id : ownOpen;
  const setOpen = (on: boolean) => {
    if (host) host.setOpen(on ? id : host.open === id ? null : host.open);
    else setOwnOpen(on);
  };

  const rowRef = useRef<HTMLButtonElement | null>(null);
  const panelRef = useRef<HTMLDivElement | null>(null);
  /** Opened from the keyboard: the first choice takes focus. */
  const focusFirst = useRef(false);
  const [box, setBox] = useState<{ left: number; top: number } | null>(null);

  useLayoutEffect(() => {
    if (!open) {
      setBox(null);
      return;
    }
    const row = rowRef.current;
    const panel = panelRef.current;
    if (!row || !panel) return;
    const scale = documentScale();
    // Where the panel is now, in visual pixels, against where it should be:
    // the difference, in CSS pixels, is the correction. Measured rather than
    // assumed so that a containing block other than the viewport — and CSS
    // zoom — are both accounted for.
    const r = row.getBoundingClientRect();
    const p = panel.getBoundingClientRect();
    const want = subMenuPosition(r, p.width, p.height, window.innerWidth, window.innerHeight);
    const current = box ?? { left: 0, top: 0 };
    setBox({
      left: current.left + (want.left - p.left) / scale,
      top: current.top + (want.top - p.top) / scale,
    });
    // `box` is deliberately not a dependency: it is the output, and measuring
    // again after it moved would correct a correction.
  }, [open]);

  // Focus waits for the panel to be placed: until then it is hidden, and a
  // hidden element cannot take focus.
  useLayoutEffect(() => {
    if (!box || !focusFirst.current) return;
    focusFirst.current = false;
    panelRef.current?.querySelector<HTMLElement>(ITEM)?.focus();
  }, [box]);

  const openFromKeys = () => {
    focusFirst.current = true;
    if (open) panelRef.current?.querySelector<HTMLElement>(ITEM)?.focus();
    else setOpen(true);
  };

  const onRowKey = (e: KeyboardEvent<HTMLButtonElement>) => {
    if (e.key === "ArrowRight" || e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      e.stopPropagation();
      openFromKeys();
    }
  };

  /** The panel's own keys. Stopped here, so the parent menu's arrows do not
   *  also move, and so Escape closes this panel and not the whole menu —
   *  one press, one thing, as everywhere else. */
  const onPanelKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const items = [...(panelRef.current?.querySelectorAll<HTMLElement>(ITEM) ?? [])];
    const at = items.indexOf(document.activeElement as HTMLElement);
    const back = () => {
      setOpen(false);
      rowRef.current?.focus();
    };
    switch (e.key) {
      case "ArrowDown":
      case "ArrowUp": {
        if (items.length === 0) break;
        const step = e.key === "ArrowDown" ? 1 : -1;
        const next = at < 0 ? (step > 0 ? 0 : items.length - 1) : (at + step + items.length) % items.length;
        items[next]?.focus();
        break;
      }
      case "Home":
        items[0]?.focus();
        break;
      case "End":
        items[items.length - 1]?.focus();
        break;
      case "ArrowLeft":
      case "Escape":
        back();
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  };

  return (
    <div className="submenu">
      <button
        ref={rowRef}
        className={`tb-menu-item submenu-row ${open ? "open" : ""}`}
        role="menuitem"
        aria-haspopup="menu"
        aria-expanded={open}
        data-submenu={id}
        onPointerEnter={() => setOpen(true)}
        onClick={() => setOpen(true)}
        onKeyDown={onRowKey}
      >
        <span className="tb-menu-label">{label}</span>
        {value ? <span className="tb-menu-hint" data-tip-overflow="">{value}</span> : null}
        <ChevronRight className="submenu-chevron" />
      </button>
      {open ? (
        <div
          ref={panelRef}
          className="ctx-menu ctx-sub"
          role="menu"
          aria-label={label}
          // Off screen until measured, so it never flashes at the corner.
          style={box ? { left: box.left, top: box.top } : { left: 0, top: 0, visibility: "hidden" }}
          onKeyDown={onPanelKey}
        >
          {children}
        </div>
      ) : null}
    </div>
  );
}

/** A choice of several, ticked when it is the current one. */
export function RadioItem({
  checked,
  label,
  detail,
  disabled = false,
  onSelect,
}: {
  checked: boolean;
  label: string;
  detail?: string;
  /** Listed, and not to be chosen here. */
  disabled?: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      className="tb-menu-item"
      role="menuitemradio"
      aria-checked={checked}
      aria-disabled={disabled || undefined}
      disabled={disabled}
      onClick={onSelect}
    >
      <span className="ctx-check">{checked ? <Check className="ctx-check-i" /> : null}</span>
      <span className="tb-menu-label">{label}</span>
      {detail ? (
        <span className="tb-menu-hint" data-tip-overflow="">
          {detail}
        </span>
      ) : null}
    </button>
  );
}

/** An on/off item, ticked when on, with its key to the right. */
export function CheckItem({
  checked,
  label,
  hint,
  onSelect,
}: {
  checked: boolean;
  label: string;
  hint?: string;
  onSelect: () => void;
}) {
  return (
    <button
      className="tb-menu-item"
      role="menuitemcheckbox"
      aria-checked={checked}
      onClick={onSelect}
    >
      <span className="ctx-check">{checked ? <Check className="ctx-check-i" /> : null}</span>
      <span className="tb-menu-label">{label}</span>
      {hint ? <span className="tb-menu-hint">{hint}</span> : null}
    </button>
  );
}
