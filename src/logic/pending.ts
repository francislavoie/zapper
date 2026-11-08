// One at a time per key: a second Play on a title whose open is still being
// set up (its place read, the player asked) is ignored, so a double-click
// cannot start two opens. Pure; exercised by src/__checks__/pending.test.mjs.

export function createPending() {
  const open = new Set<string>();
  return {
    /** True when the caller now holds `key`; false when it is already held. */
    claim(key: string): boolean {
      if (open.has(key)) return false;
      open.add(key);
      return true;
    },
    release(key: string): void {
      open.delete(key);
    },
  };
}
