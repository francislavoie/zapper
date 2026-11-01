// What to say when a stream stops by itself.
//
// The backend works out why (`ipc::ending` in Rust): most often a provider
// that allows a limited number of connections ended this one because another
// device opened a stream on the same account. This turns that into words for
// the picture, which until now simply went blank.
//
// Nothing tunes the channel again by itself after the provider ended or
// refused it. A retry against a provider at its connection limit takes the
// stream from the other device, which is the very thing that just happened
// here — so Retry is a button, pressed by a person who knows whether the
// other device matters, and where the other device is the likely reason it
// says so: "Watch here anyway". The one thing the backend reconnects by
// itself is a network failure, and only with a connection to spare
// (`connections` in Rust); `reconnectMessage` says so while it does.
//
// Pure, and checked in src/__checks__/ending.test.mjs.

import type { PlayerStateDto, SlotsDto } from "../types";

/** What a stream on its way to playing is doing, in a few words: the line
 *  under the channel's name while there is no programme to show yet. `null`
 *  once it plays, or when it has stopped (see `endedMessage`). */
export function startingLine(
  p: Pick<PlayerStateDto, "state" | "joiningLive"> & Partial<Pick<PlayerStateDto, "reconnect">>,
): string | null {
  // Waiting to reconnect, or making the attempt: the same words throughout.
  if (p.reconnect) return `Reconnecting… (${p.reconnect.attempt} of ${p.reconnect.of})`;
  if (p.state !== "opening") return null;
  // The first frame is up and held while the connect burst arrives; it
  // starts moving at live.
  if (p.joiningLive) return "Joining live…";
  return "Opening…";
}

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
    case "inUse":
      return {
        title: `Your account is in use on another device${slotsText(e.slots)}.`,
        detail: "Stop it there and retry, or watch here instead.",
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

/** " (1 of 1 connections)", or nothing when the count is unknown. */
function slotsText(slots: SlotsDto | null | undefined): string {
  if (!slots) return "";
  return ` (${slots.active} of ${slots.max} connection${slots.max === 1 ? "" : "s"})`;
}

/** What the picture says while the backend reconnects by itself after a
 *  network failure, or `null` when it is not. The reason stays under it, so
 *  "Reconnecting…" is never a mystery. */
export function reconnectMessage(
  p: Pick<PlayerStateDto, "state" | "ending" | "error" | "reconnect">,
): EndedMessage | null {
  const r = p.reconnect;
  if (!r) return null;
  const why = p.ending ? endedMessage({ ...p, state: "failed" })?.title ?? null : null;
  return {
    title: `Reconnecting… (${r.attempt} of ${r.of})`,
    detail: r.checking ? "Checking that your account has a connection free." : why,
    code: null,
  };
}

/** The buttons under a stopped picture, and what the main one does. */
export interface EndedActions {
  /** "retry" tunes again; "watchHere" tunes again knowing it will take the
   *  connection from the other device; "cancel" stops a reconnect. */
  primary: "retry" | "watchHere" | "cancel";
  /** Said under the buttons: what pressing the main one will do to somebody
   *  else. */
  note: string | null;
  dismiss: boolean;
}

/** Which buttons a stopped (or reconnecting) picture offers.
 *
 *  Where the likely reason is another device on the account — the provider
 *  ended the stream, refused one, answered with something other than video,
 *  or the account was counted full — tuning again takes the connection from
 *  that device, and the button says so rather than calling itself Retry.
 *  Everything else is a plain Retry. */
export function endedActions(
  p: Pick<PlayerStateDto, "state" | "ending" | "reconnect">,
): EndedActions | null {
  if (p.reconnect) return { primary: "cancel", note: null, dismiss: false };
  if (p.state !== "failed") return null;
  const e = p.ending;
  if (!e) return { primary: "retry", note: null, dismiss: true };
  switch (e.kind) {
    case "inUse":
      return { primary: "watchHere", note: "This will stop playback on the other device.", dismiss: true };
    case "refused":
      return e.limit
        ? { primary: "watchHere", note: "This will stop playback on the other device.", dismiss: true }
        : {
            primary: "watchHere",
            note: "If another device is using your connection, this will stop it there.",
            dismiss: true,
          };
    case "dropped":
    case "notVideo":
      return {
        primary: "watchHere",
        note: "If another device is using your connection, this will stop it there.",
        dismiss: true,
      };
    default:
      return { primary: "retry", note: null, dismiss: true };
  }
}
