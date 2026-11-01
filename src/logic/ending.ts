// What to say when a stream stops by itself.
//
// The backend works out why (`ipc::ending` in Rust): most often a provider
// that allows a limited number of connections ended this one because another
// device opened a stream on the same account. This turns that into words for
// the picture, which until now simply went blank.
//
// Nothing here, or anywhere, tunes the channel again by itself. A retry
// against a provider at its connection limit takes the stream from the other
// device, which is the very thing that just happened here — so Retry is a
// button, pressed by a person who knows whether the other device matters.
//
// Pure, and checked in src/__checks__/ending.test.mjs.

import type { PlayerStateDto } from "../types";

export interface EndedMessage {
  /** The headline on the picture. */
  title: string;
  /** What probably happened, or what to try. */
  detail: string | null;
  /** The server's own answer, for whoever wants it: "HTTP 509". */
  code: string | null;
}

/** The message for a stream that is not playing because it failed, or `null`
 *  while there is nothing to explain. */
export function endedMessage(
  p: Pick<PlayerStateDto, "state" | "ending" | "error">,
): EndedMessage | null {
  if (p.state !== "failed") return null;
  const e = p.ending;
  const code = e?.status != null ? `HTTP ${e.status}` : null;
  if (!e) {
    // Failed without an account of why: the open could not be sent, or a
    // source explained it in its own words (an HDHomeRun's busy tuners),
    // which are better than any guess here.
    return { title: "Couldn't play this channel.", detail: p.error, code: null };
  }
  switch (e.kind) {
    case "dropped":
      return {
        title: "Playback stopped — your provider ended this stream.",
        detail: "Another device may be using your connection.",
        code,
      };
    case "refused":
      return e.limit
        ? {
            title: "Your provider refused the stream: too many connections.",
            detail: "Another device is using your connection. Stop it there, then retry.",
            code,
          }
        : {
            title: "Your provider refused the stream (too many connections?)",
            detail: "Another device may be using your connection.",
            code,
          };
    case "notVideo":
      return {
        title: "Your provider sent something other than video.",
        detail: "Providers often answer this way when too many connections are open.",
        code,
      };
    case "unavailable":
      return {
        title: "This channel isn't available from your provider.",
        detail: "It may have been removed. Refreshing the channel list can help.",
        code,
      };
    case "serverError":
      return {
        title: "Your provider's server had a problem.",
        detail: "It may be busy. Try again in a moment.",
        code,
      };
    case "network":
      return {
        title: "Couldn't reach the server.",
        detail: "Check your network connection, then retry.",
        code,
      };
    case "noVideo":
      return {
        title: "The channel didn't start.",
        detail: "No picture arrived in time. It may be off the air.",
        code,
      };
    case "other":
      return {
        title: "Couldn't play this channel.",
        // An open that fails without a word is, on a provider at its limit,
        // the commonest way of saying no — but only "may".
        detail: "The server didn't say why. If another device is watching on this account, your provider may be refusing a second connection.",
        code,
      };
  }
}
