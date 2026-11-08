// A filter box with a clear button inside its right edge.
//
// Without it, clearing means select-all-and-delete or Escape, neither of
// which a box shows you it has. The ✕ appears only while there is text to clear,
// sits inside the box so it costs no width, and puts focus back in the box,
// because the next thing after clearing a search is usually typing another.

import type { KeyboardEventHandler, Ref } from "react";
import { useRef } from "react";

export function SearchField({
  value,
  onChange,
  placeholder,
  className,
  inputRef,
  onKeyDown,
  clearLabel = "Clear",
  keyHint,
}: {
  value: string;
  onChange: (text: string) => void;
  placeholder?: string;
  /** Sizing for the box as a whole, e.g. a flex basis. */
  className?: string;
  inputRef?: Ref<HTMLInputElement>;
  onKeyDown?: KeyboardEventHandler<HTMLInputElement>;
  clearLabel?: string;
  /** The key that focuses this box, drawn as a key-cap at its right end
   *  while the box is empty and unfocused (CSS decides when). */
  keyHint?: string;
}) {
  const own = useRef<HTMLInputElement | null>(null);
  const setRefs = (el: HTMLInputElement | null) => {
    own.current = el;
    if (typeof inputRef === "function") inputRef(el);
    else if (inputRef) (inputRef as { current: HTMLInputElement | null }).current = el;
  };

  return (
    <span className={`search-field ${className ?? ""}`}>
      <input
        className="field"
        ref={setRefs}
        placeholder={placeholder}
        spellCheck={false}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
      />
      {keyHint ? (
        <kbd className="search-hint" aria-hidden="true">
          {keyHint}
        </kbd>
      ) : null}
      {value !== "" ? (
        <button
          type="button"
          className="search-clear"
          aria-label={clearLabel}
          data-tip={clearLabel}
          // Keeps focus in the box: a mousedown on a button would take it.
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => {
            onChange("");
            own.current?.focus();
          }}
        >
          <svg viewBox="0 0 12 12" aria-hidden="true">
            <path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
        </button>
      ) : null}
    </span>
  );
}
