// What Settings → Account says about the account's connections.
//
// Pure; checked in src/__checks__/connections.test.mjs.

import type { ConnectionsDto } from "../types";
import { calendarDate } from "./format.ts";

/** "1 of 1 in use · expires 1 Mar 2027", from what the backend knows. */
export function connectionsLine(c: ConnectionsDto): string {
  const parts: string[] = [];
  if (c.counted && c.max != null && c.active != null) {
    parts.push(`${c.active} of ${c.max} in use`);
  } else if (c.max != null) {
    // Nobody can count them: say what is allowed, and what is ours.
    parts.push(`${c.max} allowed · ${c.ours} in use here`);
  } else {
    parts.push("No limit reported");
  }
  if (c.expires != null) parts.push(`expires ${calendarDate(c.expires)}`);
  return parts.join(" · ");
}
