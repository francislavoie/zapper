// How a favourite says what it is: how long it has been watched, and how long
// ago that was. Pure; exercised by src/__checks__/favourites.test.mjs.
//
// Everything here goes through `Intl`. Hand-rolling "4h 20m" is three lines
// and wrong the moment the interface is asked to speak anything but English —
// and SettingsDto already carries a language tag for exactly that day, so the
// formatting must be the part that does not need rewriting. `Intl` also knows
// things a hand-rolled version does not: that "yesterday" is a word, and that
// plenty of locales do not put the number first.
//
// `Intl.NumberFormat` with `style: "unit"` rather than `Intl.DurationFormat`,
// which is newer than the WebKit this app ships against on some distributions
// and would be a blank label rather than a fallback.

/** Formatters are not cheap to build and these are called per row, per
 *  render. Keyed by everything that varies. */
const numberFormats = new Map<string, Intl.NumberFormat>();
const relativeFormats = new Map<string, Intl.RelativeTimeFormat>();

function unitFormat(
  locale: string | undefined,
  unit: "hour" | "minute" | "second",
  unitDisplay: "long" | "short" | "narrow",
): Intl.NumberFormat {
  const key = `${locale ?? ""}|${unit}|${unitDisplay}`;
  let f = numberFormats.get(key);
  if (!f) {
    f = new Intl.NumberFormat(locale, {
      style: "unit",
      unit,
      unitDisplay,
      maximumFractionDigits: 0,
    });
    numberFormats.set(key, f);
  }
  return f;
}

function relativeFormat(locale: string | undefined): Intl.RelativeTimeFormat {
  const key = locale ?? "";
  let f = relativeFormats.get(key);
  if (!f) {
    // "auto" is what turns -1 day into "yesterday" instead of "1 day ago".
    // Short, to match the watch time beside it: "Watched 5 min" next to
    // "10 minutes ago" reads as two different clocks.
    f = new Intl.RelativeTimeFormat(locale, { numeric: "auto", style: "short" });
    relativeFormats.set(key, f);
  }
  return f;
}

/** Seconds, said the way a person would: "4h 20m", "12 min", "45 sec".
 *
 *  Abbreviated, because the rail is 16rem wide and this sits beside a
 *  channel name: "4 hours, 20 minutes" there is an ellipsis, and the row is
 *  scanned rather than read. Short units below an hour, where there is room
 *  for a space and a word; narrow above, where two units have to share the
 *  line. Truncated rather than rounded, so a channel never claims a minute
 *  it has not been watched.
 *
 *  The exact abbreviation is the locale's: en-US says "5 min", en-GB says
 *  "5 mins", and both are right where they are spoken. */
export function watchTimeLabel(seconds: number, locale?: string): string {
  const s = Number.isFinite(seconds) ? Math.max(0, Math.floor(seconds)) : 0;
  if (s < 60) return unitFormat(locale, "second", "short").format(s);
  if (s < 3600) return unitFormat(locale, "minute", "short").format(Math.floor(s / 60));
  const hours = Math.floor(s / 3600);
  const minutes = Math.floor((s % 3600) / 60);
  const h = unitFormat(locale, "hour", "narrow").format(hours);
  if (minutes === 0) return h;
  return `${h} ${unitFormat(locale, "minute", "narrow").format(minutes)}`;
}

/** The watch time with its label: "Watched 5 min".
 *
 *  A bare duration beside a channel name is ambiguous — it could as easily
 *  be how long the programme runs — and this column is the reason the
 *  channel is in the list at all. */
export function watchedLabel(seconds: number, locale?: string): string {
  return `Watched ${watchTimeLabel(seconds, locale)}`;
}

/** How long ago something was last watched: "now", "20 minutes ago",
 *  "yesterday", "3 months ago". Empty string when it has never been watched,
 *  which is a thing the caller decides how to say.
 *
 *  A timestamp in the future reads as "now" rather than as a countdown — the
 *  same clamp the backend's score applies, and for the same reason: clocks
 *  change, and a database outlives any one boot. */
/** Drops the full stop an abbreviated relative time ends its unit with.
 *
 *  `Intl.RelativeTimeFormat` in the short style says "10 min. ago", and the
 *  stop reads as a typo beside "Watched 5 min" — which is the same
 *  abbreviation without one, because `Intl.NumberFormat` does not add it.
 *  Only stops followed by a space or the end are touched, so a decimal
 *  cannot be mangled.
 *
 *  English-shaped, and it should be revisited with the translations: a
 *  locale that punctuates abbreviations differently deserves better than
 *  having them stripped. */
function tidy(s: string): string {
  return s.replace(/\.(?=\s|$)/g, "");
}

export function lastWatchedLabel(
  lastWatched: number | null,
  nowSeconds: number,
  locale?: string,
): string {
  if (lastWatched === null || !Number.isFinite(lastWatched)) return "";
  const ago = Math.max(0, Math.round(nowSeconds - lastWatched));
  const rtf = relativeFormat(locale);
  const said = (n: number, unit: Intl.RelativeTimeFormatUnit) => tidy(rtf.format(n, unit));
  if (ago < 45) return said(0, "second");
  if (ago < 3600) return said(-Math.round(ago / 60), "minute");
  if (ago < 86_400) return said(-Math.round(ago / 3600), "hour");
  if (ago < 2_592_000) return said(-Math.round(ago / 86_400), "day");
  if (ago < 31_536_000) return said(-Math.round(ago / 2_592_000), "month");
  return said(-Math.round(ago / 31_536_000), "year");
}

/** The one line under a favourite's name: what it has earned its place with,
 *  and when. */
export function favouriteMeta(
  watchSeconds: number,
  lastWatched: number | null,
  nowSeconds: number,
  locale?: string,
): string {
  const when = lastWatchedLabel(lastWatched, nowSeconds, locale);
  const watched = watchTimeLabel(watchSeconds, locale);
  return when === "" ? watched : `${watched} · ${when}`;
}

export interface MenuBox {
  left: number;
  top: number;
}

/** Where a context menu goes so that all of it is on screen.
 *
 *  It opens down and to the right of the pointer, which is what every desktop
 *  does, and flips back inside the window rather than running off it. The
 *  margin keeps it clear of the window's own resize ring — the window is
 *  undecorated, and its outer few pixels belong to the grab. */
export function menuPosition(
  x: number,
  y: number,
  width: number,
  height: number,
  viewportWidth: number,
  viewportHeight: number,
  margin = 6,
): MenuBox {
  const maxLeft = viewportWidth - width - margin;
  const maxTop = viewportHeight - height - margin;
  return {
    left: Math.max(margin, Math.min(x, maxLeft)),
    top: Math.max(margin, Math.min(y, maxTop)),
  };
}

/** How many favourites the rail asks for.
 *
 *  More than fills the rail at any zoom, and short enough that the backend's
 *  join and the ranking behind it stay trivial.
 *
 *  Here rather than in the component because a module that exports a
 *  component and a constant cannot be hot-reloaded — see
 *  src/__checks__/fast-refresh.test.mjs. */
export const FAVOURITE_LIMIT = 40;
