// Posters that failed once are not asked for again this session: a great
// many are missing or 404, and a list of tens of thousands would ask again
// for each as it scrolls back. Kept by address, so forgotten when the
// movies are another account's (whose addresses may be the same ids').

const dead = new Set<string>();

export function isDeadPoster(url: string): boolean {
  return dead.has(url);
}

export function markDeadPoster(url: string): void {
  dead.add(url);
}

/** Asks for every poster again: the movies are another account's now. */
export function clearDeadPosters(): void {
  dead.clear();
}
