// Settings › Recording › File names: a preset, or a pattern of one's own,
// and what either makes of two example programmes — one with a season and
// episode, one without — as the full path the service would write.
//
// A custom pattern is checked as it is typed, with the service's own words
// (src/logic/naming.ts), and only sent once it is valid; until then the
// field keeps what was typed and says what is wrong, and the service keeps
// the last good one.

import { useRef, useState } from "react";
import {
  NAMING_EXAMPLES,
  NAMING_PRESETS,
  TOKENS,
  TOKEN_HELP,
  patternFor,
  previewPath,
  validatePattern,
} from "../logic/naming";
import type { Naming, RecordingSettingsDto } from "../types";
import { Segmented } from "./SettingsParts";

export function NamingField({
  settings,
  patch,
}: {
  settings: RecordingSettingsDto;
  patch: (p: Partial<RecordingSettingsDto>) => void;
}) {
  const naming: Naming = settings.naming ?? "mediaServer";
  const saved = settings.namingPattern ?? "";
  /** What is in the field while it differs from what was sent: typed but
   *  not valid yet. */
  const [draft, setDraft] = useState<string | null>(null);
  const input = useRef<HTMLInputElement | null>(null);
  const custom = naming === "custom";
  const text = draft ?? saved;
  const why = custom ? validatePattern(text) : null;
  const pattern = patternFor(naming, custom && !why ? text : saved);

  const choose = (id: Naming) => {
    setDraft(null);
    if (id === "custom" && !saved.trim()) {
      // Start from the layout chosen so far: easier to adjust than to write.
      patch({ naming: id, namingPattern: patternFor(naming, saved) });
    } else {
      patch({ naming: id });
    }
  };

  const type = (v: string) => {
    if (validatePattern(v) === null) patch({ naming: "custom", namingPattern: v.trim() });
    // What was typed stays in the field, spaces at the ends and all, until
    // it leaves; the service has the trimmed one.
    setDraft(v === v.trim() && validatePattern(v) === null ? null : v);
  };

  const insert = (token: string) => {
    const el = input.current;
    const at = el?.selectionStart ?? text.length;
    const end = el?.selectionEnd ?? at;
    const t = `{${token}}`;
    type(text.slice(0, at) + t + text.slice(end));
    requestAnimationFrame(() => {
      el?.focus();
      el?.setSelectionRange(at + t.length, at + t.length);
    });
  };

  return (
    <div className="naming">
      <Segmented<Naming>
        label="File names"
        value={naming}
        options={NAMING_PRESETS}
        onChange={choose}
      />
      {custom ? (
        <div className="naming-custom">
          <input
            id="rec-pattern"
            ref={input}
            className="field naming-pattern"
            value={text}
            spellCheck={false}
            autoComplete="off"
            aria-label="File name pattern"
            aria-invalid={why ? true : undefined}
            aria-describedby="rec-pattern-why"
            onChange={(e) => type(e.target.value)}
            onBlur={() => !why && setDraft(null)}
          />
          <div className="naming-tokens" aria-label="Tokens">
            {TOKENS.map((t) => (
              <button
                key={t}
                type="button"
                className="naming-token"
                data-tip={TOKEN_HELP[t]}
                // Keep the field's caret where it was, for the token to go.
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => insert(t)}
              >
                {`{${t}}`}
              </button>
            ))}
            <span className="naming-hint">/ makes a folder</span>
          </div>
        </div>
      ) : null}
      {why ? (
        <div id="rec-pattern-why" className="naming-why" role="alert">
          {/* The service's words, which start lower-case to follow
              "file name pattern:" in an error; here they stand alone. */}
          {why.charAt(0).toUpperCase() + why.slice(1)}.
        </div>
      ) : (
        <ul className="naming-preview" aria-label="For example">
          {NAMING_EXAMPLES.map((ex) => (
            <li key={ex.programme.title}>
              {previewPath(settings.outputDir, pattern, ex.programme, ex.start)}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
