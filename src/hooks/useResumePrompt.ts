// The Resume / Start over question, as the app runs it. Mounted once, by
// App: the question is asked while the player holds the title the prompt
// names (opening or paused), and goes when the title plays, fails or stops,
// or the player moves to anything else (logic/resume.ts `promptStep`).
// Seen-ness belongs to the prompt asked: a new one starts unseen. Up next
// stands aside for the held title until it has played (`held`), even once
// Escape or a seek has closed the question (logic/resume.ts `holdStep`).

import { useEffect, useRef } from "react";
import { holdStep, holding, promptStep, seenFor, type Hold, type ResumePrompt } from "../logic/resume";
import { useApp, useDispatch } from "../store";

export function useResumePrompt(): { prompt: ResumePrompt | null; shown: boolean; held: boolean } {
  const { player, continue: cont } = useApp();
  const dispatch = useDispatch();
  const prompt = cont.prompt;
  const last = useRef<{ prompt: ResumePrompt | null; seen: boolean }>({ prompt: null, seen: false });
  const lastHold = useRef<Hold | null>(null);
  const seen = seenFor(last.current.prompt, last.current.seen, prompt);
  const step = promptStep(prompt, seen, player);
  const hold = holdStep(lastHold.current, prompt, player);
  useEffect(() => {
    last.current = { prompt, seen: step.seen };
    lastHold.current = hold;
    if (step.clear) dispatch({ type: "continue/prompt", prompt: null });
  });
  return { prompt, shown: step.shown, held: holding(hold) };
}
