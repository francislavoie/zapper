// The channel logo, and the memory of which ones do not exist.
//
// Shared by the channel list and the favourites rail rather than duplicated,
// so the cache of dead URLs is shared too: provider logos 404 constantly,
// and a favourite is usually also a row in the list.

import { useState } from "react";

/** Logo URLs 404 constantly in large catalogues. One failure per URL is enough. */
const deadLogos = new Set<string>();

export function Logo({ url, className = "row-logo" }: { url: string | null; className?: string }) {
  const [broken, setBroken] = useState(false);
  if (!url || broken || deadLogos.has(url)) {
    return <span className={`${className} blank`} aria-hidden />;
  }
  return (
    <img
      className={className}
      src={url}
      alt=""
      loading="lazy"
      decoding="async"
      onError={() => {
        deadLogos.add(url);
        setBroken(true);
      }}
    />
  );
}
