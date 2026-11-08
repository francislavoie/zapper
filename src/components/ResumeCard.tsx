// The Resume / Start over question, over the picture of a title opened held
// at its place (useResumePrompt asks; this draws it). In the full player it
// is centred with Resume focused, and Enter and Escape answer for it
// (useGlobalKeys); docked (compact) it is small, takes no focus, and its
// buttons are in the tab order.

import { useEffect, useRef } from "react";
import { resumeHeld, startOverHeld } from "../actions";
import { promptLine, promptStep, resumeLabel } from "../logic/resume";
import { useApp, useDispatch } from "../store";

export function ResumeCard({ compact = false }: { compact?: boolean }) {
  const { player, continue: cont } = useApp();
  const dispatch = useDispatch();
  const prompt = cont.prompt;
  const shown = prompt !== null && promptStep(prompt, false, player).shown;
  const resumeBtn = useRef<HTMLButtonElement | null>(null);
  useEffect(() => {
    if (shown && !compact) resumeBtn.current?.focus({ preventScroll: true });
  }, [shown, compact]);
  if (!prompt || !shown) return null;
  return (
    <div
      className={`resume-prompt ${compact ? "compact" : ""}`}
      role="dialog"
      aria-label={`Resume ${prompt.title}?`}
      // Its clicks are its own: not a pause, nor the docked picture's way
      // into the player.
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.stopPropagation()}
    >
      <h4 className="resume-prompt-title" data-tip-overflow={`Resume ${prompt.title}?`}>
        Resume {prompt.title}?
      </h4>
      {compact ? null : <p className="resume-prompt-line">{promptLine(prompt, Math.floor(Date.now() / 1000))}</p>}
      <div className="resume-prompt-btns">
        <button
          ref={resumeBtn}
          className="ctl primary resume-prompt-resume"
          onClick={() => void resumeHeld(dispatch)}
        >
          ▶ {resumeLabel(prompt.position)}
        </button>
        <button className="ctl resume-prompt-start" onClick={() => void startOverHeld(dispatch)}>
          ↺ Start over
        </button>
      </div>
    </div>
  );
}
