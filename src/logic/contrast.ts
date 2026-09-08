// Is a surface light or dark, and is it opaque?
//
// Two questions the app has to be able to answer about itself, because both
// have a failure mode nobody notices by looking at a screenshot for a second:
// a stray white panel in a dark app, and an opaque pixel over the video plane
// where mpv is drawing. `src/audit.ts` walks the DOM with these; they are pure
// and checked in src/__checks__/contrast.test.mjs.

export interface Rgba {
  r: number;
  g: number;
  b: number;
  a: number;
}

/** `rgb(…)`, `rgba(…)`, `#rgb`, `#rrggbb`, and the one keyword that matters.
 *  Anything unrecognised is `null` — the caller must not guess. */
export function parseCssColor(input: string): Rgba | null {
  const s = input.trim().toLowerCase();
  if (s === "" || s === "transparent") return { r: 0, g: 0, b: 0, a: 0 };
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/.exec(s);
  if (hex) {
    const h = hex[1];
    const full =
      h.length === 3 ? `${h[0]}${h[0]}${h[1]}${h[1]}${h[2]}${h[2]}` : h;
    return {
      r: parseInt(full.slice(0, 2), 16),
      g: parseInt(full.slice(2, 4), 16),
      b: parseInt(full.slice(4, 6), 16),
      a: 1,
    };
  }
  const fn = /^rgba?\(([^)]+)\)$/.exec(s);
  if (!fn) return null;
  const parts = fn[1].split(/[\s,/]+/).filter((p) => p !== "");
  if (parts.length < 3) return null;
  const n = parts.slice(0, 3).map(Number);
  if (n.some((v) => !Number.isFinite(v))) return null;
  const a = parts.length > 3 ? Number(parts[3]) : 1;
  return { r: n[0], g: n[1], b: n[2], a: Number.isFinite(a) ? a : 1 };
}

/** WCAG relative luminance, 0 (black) to 1 (white). */
export function relativeLuminance({ r, g, b }: Omit<Rgba, "a">): number {
  const f = (c: number) => {
    const v = Math.min(1, Math.max(0, c / 255));
    return v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
  };
  return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
}

/** A surface bright enough to read as "a white panel", and solid enough to be
 *  a surface at all. A faint translucent wash over a dark app is not one. */
export function isLightSurface(color: string, threshold = 0.5): boolean {
  const c = parseCssColor(color);
  if (!c || c.a < 0.5) return false;
  return relativeLuminance(c) > threshold;
}

/** Anything at all is painted here — the test that matters over the video
 *  plane, where even a 4% wash is a visible film across the picture. */
export function paintsAnything(color: string): boolean {
  const c = parseCssColor(color);
  return c === null ? true : c.a > 0.001;
}
