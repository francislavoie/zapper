// The one place that asks what machine this is, once, at load.
//
// Only what differs is decided by it: the shortcut modifier and how shortcuts
// are written (logic/keys.ts), and on a Mac the title bar, which has the
// system's own window buttons. The page also gets `data-platform="mac"` on
// <html>, for the few rules that are the stylesheet's business.

import { isMacPlatform } from "./logic/keys";

interface NavigatorWithUAData extends Navigator {
  userAgentData?: { platform?: string };
}

const nav: NavigatorWithUAData | undefined =
  typeof navigator === "undefined" ? undefined : (navigator as NavigatorWithUAData);

/** True on macOS (or with `?platform=mac` in the browser mock). */
export const IS_MAC: boolean = isMacPlatform({
  uaPlatform: nav?.userAgentData?.platform,
  platform: nav?.platform,
  userAgent: nav?.userAgent,
  search: typeof location === "undefined" ? "" : location.search,
});

if (IS_MAC && typeof document !== "undefined") {
  document.documentElement.dataset.platform = "mac";
}
