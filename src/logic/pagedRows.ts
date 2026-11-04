// The page cache behind a list that knows its length from the first answer
// (the Movies list; Live TV's channel list does not, and has its own).
//
// Pure, so that what must hold can be checked: an answer to a question asked
// about another list, another sort or an earlier revision of this one never
// lands in the current list. Each reset hands out a new generation, every
// request carries the one it was asked in, and a response with an older one is
// dropped.
//
// A refresh of the same list (a mark, a sync) keeps what was on screen until
// each page's replacement arrives, so it does not blank the list or move its
// scroll position. Another list starts empty.

import { PAGE_SIZE, missingPages, pageOf } from "./virtual.ts";

export class PageBook<T> {
  private key: string | null = null;
  private revision = 0;
  private generation = 0;
  private pages = new Map<number, T[]>();
  private stale = new Map<number, T[]>();
  private inflight = new Set<number>();
  private failed = new Set<number>();
  /** The list's length, once any page of it has answered. */
  private exact: number | null = null;
  /** The length the list claimed before a refresh, shown until the new one
   *  is known. */
  private staleTotal: number | null = null;
  error: string | null = null;
  private listeners = new Set<() => void>();

  /** Called whenever an answer changes what the book holds, so every list
   *  drawing from it redraws. */
  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => void this.listeners.delete(fn);
  }

  private notify(): void {
    for (const fn of this.listeners) fn();
  }

  /** Moves to `key` at `revision`; true when that reset the book. */
  sync(key: string, revision: number): boolean {
    if (this.key === key && this.revision === revision) return false;
    if (this.key === key) {
      // Pages fetched before this refresh outrank anything older still kept.
      for (const [p, rows] of this.pages) this.stale.set(p, rows);
      this.staleTotal = this.total;
    } else {
      this.stale = new Map();
      this.staleTotal = null;
    }
    this.key = key;
    this.revision = revision;
    this.pages = new Map();
    this.inflight = new Set();
    this.failed = new Set();
    this.exact = null;
    this.error = null;
    this.generation += 1;
    return true;
  }

  /** The number to ask a page with, and to hand back with its answer. */
  get ticket(): number {
    return this.generation;
  }

  get total(): number {
    return this.exact ?? this.staleTotal ?? 0;
  }

  get loading(): boolean {
    return this.inflight.size > 0;
  }

  /** The pages to ask for so that rows `start` to `end` can be drawn. Before
   *  the length is known, page 0 as well: it is what says how long the list
   *  is, and a refresh scrolled far down needs it to learn the list got
   *  shorter. Pages already held, in flight or failed are left alone, so a
   *  scroll does not ask again. */
  want(start: number, end: number): number[] {
    const known = (p: number) =>
      this.pages.has(p) || this.inflight.has(p) || this.failed.has(p);
    const out = missingPages(start, end, PAGE_SIZE, known);
    if (this.exact === null && !known(0) && !out.includes(0)) out.unshift(0);
    return out;
  }

  /** Marks a page as asked; false when it is already, or `ticket` is old. */
  begin(ticket: number, page: number): boolean {
    if (ticket !== this.generation || this.inflight.has(page)) return false;
    this.inflight.add(page);
    return true;
  }

  /** Takes an answer; false, and nothing changes, when it is to an old
   *  question. */
  accept(ticket: number, page: number, total: number, items: T[]): boolean {
    if (ticket !== this.generation) return false;
    this.inflight.delete(page);
    this.pages.set(page, items);
    this.stale.delete(page);
    this.exact = total;
    this.staleTotal = null;
    for (const p of [...this.stale.keys()]) {
      if (p * PAGE_SIZE >= total) this.stale.delete(p);
    }
    this.error = null;
    this.notify();
    return true;
  }

  /** Remembers a failure so a scroll does not hammer a dead backend. */
  fail(ticket: number, page: number, message: string): boolean {
    if (ticket !== this.generation) return false;
    this.inflight.delete(page);
    this.failed.add(page);
    this.error = message;
    this.notify();
    return true;
  }

  /** Lets failed pages be asked again. */
  retry(): void {
    this.failed = new Set();
    this.error = null;
  }

  rowAt(index: number): T | undefined {
    if (index < 0 || index >= this.total) return undefined;
    const p = pageOf(index, PAGE_SIZE);
    return (this.pages.get(p) ?? this.stale.get(p))?.[index % PAGE_SIZE];
  }
}
