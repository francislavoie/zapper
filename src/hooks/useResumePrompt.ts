// The Resume / Start over question, as the app runs it. Mounted once, by
// App: the question is asked while the player holds the title the prompt
// names (opening or paused), and goes when the title plays, fails or stops,
// or the player moves to anything else (logic/resume.ts `promptStep`).

import { useEffect, useRef } from "react";
import { promptStep, type ResumePrompt } from "../logic/resume";
import { useApp, useDispatch } from "../store";

export function useResumePrompt(): { prompt: ResumePrompt | null; shown: boolean } {
  const { player, continue: cont } = useApp();
  const dispatch = useDispatch();
  const prompt = cont.prompt;
  const seen = useRef(false);
  const step = promptStep(prompt, seen.current, player);
  useEffect(() => {
    seen.current = prompt === null ? false : step.seen;
    if (step.clear) {
      seen.current = false;
      dispatch({ type: "continue/prompt", prompt: null });
    }
  });
  return { prompt, shown: step.shown };
}
