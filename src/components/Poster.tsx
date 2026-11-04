// A movie's poster, and what stands in for it. Posters come from the provider
// through the service (never a provider address), a great many of them are
// missing or 404, and a list of tens of thousands must not wait for any.
// So the image is lazy and decoded off the main thread, and a title without
// one, or whose one failed, gets a coloured block with its initial: the colour
// from the title, so the same film is the same colour every time.

import { useState } from "react";

/** Posters that failed once are not asked for again this session. */
const deadPosters = new Set<string>();

/** A hue from the title's letters: stable, and spread over the wheel. */
function hueOf(name: string): number {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) % 360;
  return h;
}

export function Poster({
  url,
  name,
  className = "poster",
}: {
  url: string | null;
  /** The film's name: its initial and its colour stand in for a missing poster. */
  name: string;
  className?: string;
}) {
  const [broken, setBroken] = useState(false);
  if (!url || broken || deadPosters.has(url)) {
    const initial = name.trim().charAt(0).toUpperCase();
    return (
      <span
        className={`${className} blank`}
        style={{ background: `linear-gradient(150deg, hsl(${hueOf(name)} 38% 30%), hsl(${(hueOf(name) + 40) % 360} 40% 14%))` }}
        aria-hidden
      >
        {initial}
      </span>
    );
  }
  return (
    <img
      className={className}
      src={url}
      alt=""
      loading="lazy"
      decoding="async"
      onError={() => {
        deadPosters.add(url);
        setBroken(true);
      }}
    />
  );
}
