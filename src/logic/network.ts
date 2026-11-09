// Settings › Recording › Network access: what the box checks before it asks
// the recording service, and how it words what the service answers. The
// service checks the same things itself (crates/zapper/src/recording/access.rs
// and config.rs); these say so sooner, beside the field.
//
// Checked in src/__checks__/network.test.mjs.

import { ago, dateTime } from "./format.ts";
import type { DeviceDto, NetworkDto } from "../types";

export const DEFAULT_PORT = 8796;
export const MIN_PORT = 1024;
export const MAX_PORT = 65535;
/** Characters, not bytes, as the service counts them. */
export const MIN_PASSWORD = 8;
/** Bytes of UTF-8. */
export const MAX_PASSWORD = 1024;

/** The port typed, or null when it is not a port Settings takes. */
export function parsePort(text: string): number | null {
  const t = text.trim();
  if (!/^\d+$/.test(t)) return null;
  const n = Number(t);
  return n >= MIN_PORT && n <= MAX_PORT ? n : null;
}

/** Why the port typed cannot be used, or null. */
export function portProblem(text: string): string | null {
  if (parsePort(text) !== null) return null;
  if (!/^\s*\d+\s*$/.test(text)) return "A port is a whole number.";
  return `Choose a port between ${MIN_PORT} and ${MAX_PORT}.`;
}

/** Why the password typed (and typed again) cannot be set, or null. */
export function passwordProblem(password: string, confirm: string): string | null {
  if ([...password].length < MIN_PASSWORD) return `At least ${MIN_PASSWORD} characters.`;
  if (password.trim() === "") return "Not only spaces.";
  if (new TextEncoder().encode(password).length > MAX_PASSWORD) return "That is too long.";
  if (password !== confirm) return "The two passwords differ.";
  return null;
}

/** Network access can be turned on only once other devices have a password
 *  to sign in with; turning it off is always allowed. */
export function canEnable(n: Pick<NetworkDto, "enabled" | "passwordSet">): boolean {
  return n.enabled || n.passwordSet;
}

/** The box's state in a few words, after its title. */
export function networkHint(n: NetworkDto): string {
  if (!n.enabled) return "off";
  if (n.error) return "not working";
  return n.https ? "on · HTTPS" : "on · plain HTTP";
}

/** A device's second line: where and when it was last seen, and since when
 *  it is signed in. */
export function deviceLine(d: DeviceDto, now: number): string {
  const parts: string[] = [];
  if (d.lastUsed !== undefined) {
    parts.push(`Last used ${ago(d.lastUsed, now)}${d.address ? ` from ${d.address}` : ""}`);
  } else if (d.address) {
    parts.push(`From ${d.address}`);
  }
  parts.push(`signed in ${dateTime(d.created)}`);
  const line = parts.join(" · ");
  return line.charAt(0).toUpperCase() + line.slice(1);
}
