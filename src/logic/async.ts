// A tiny four-state async cell. Everything the backend owns can fail — the
// commands all return Result<_, String> — so every fetch site carries an
// explicit error branch rather than rendering an empty container.

export type Async<T> =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ready"; data: T }
  | { status: "error"; message: string };

export const idle = <T,>(): Async<T> => ({ status: "idle" });
export const loading = <T,>(): Async<T> => ({ status: "loading" });
export const ready = <T,>(data: T): Async<T> => ({ status: "ready", data });
export const failed = <T,>(message: string): Async<T> => ({
  status: "error",
  message,
});

/** Tauri rejects with a plain string; the browser rejects with an Error; a
 *  missing Tauri bridge rejects with something else again. Normalise all
 *  three into one line a human can read. */
export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  if (e && typeof e === "object" && "message" in e) {
    const m = (e as { message: unknown }).message;
    if (typeof m === "string") return m;
  }
  return String(e);
}
