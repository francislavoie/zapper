// Wall clock in unix seconds, re-rendering on an interval.
//
// Anything that draws "now" — the guide's now-line, a programme's progress —
// needs the page to notice that time has passed. Nothing else does, so the
// tick is opt-in and coarse.

import { useEffect, useState } from "react";

/** A test hook: a browser driver can freeze the clock to assert the now-line's
 *  position against a known instant. Never set by the app itself. */
declare global {
  interface Window {
    __zapperNow?: number;
  }
}

/** `?now=<unix seconds>` freezes the clock, the same way `?backend=broken`
 *  fakes a dead backend. There is no query string under Tauri. */
function frozen(): number | null {
  if (typeof window === "undefined") return null;
  if (typeof window.__zapperNow === "number") return window.__zapperNow;
  const m = /(?:^|[?&])now=(\d+)(?:&|$)/.exec(window.location.search);
  return m ? Number(m[1]) : null;
}

export const nowSeconds = (): number => frozen() ?? Math.floor(Date.now() / 1000);

export function useClock(intervalMs = 30_000): number {
  const [t, setT] = useState(nowSeconds);
  useEffect(() => {
    const id = window.setInterval(() => setT(nowSeconds()), intervalMs);
    return () => window.clearInterval(id);
  }, [intervalMs]);
  return t;
}
