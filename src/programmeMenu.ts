// Opening the programme menu (components/ProgrammeMenu.tsx) from anything
// that shows a programme: the guide's blocks and the programme results'
// rows. A right-click opens it at the pointer; the keyboard's menu key, or
// Shift+F10, opens it under the element and gives it focus, as the category
// rail's menu does.

import type { KeyboardEvent, MouseEvent } from "react";
import type { ChannelDto, ProgrammeDto } from "./types";

export type ProgrammeMenuOpener = (
  channel: ChannelDto,
  programme: ProgrammeDto,
  at: { x: number; y: number },
  keyboard: boolean,
) => void;

export function programmeMenuProps(
  channel: ChannelDto,
  programme: ProgrammeDto,
  open: ProgrammeMenuOpener,
) {
  return {
    onContextMenu: (e: MouseEvent<HTMLElement>) => {
      // WebKit's own menu is suppressed at the GTK level; this one replaces
      // it, so the browser's must not also appear in the mock.
      e.preventDefault();
      open(channel, programme, { x: e.clientX, y: e.clientY }, false);
    },
    onKeyDown: (e: KeyboardEvent<HTMLElement>) => {
      if (e.key !== "ContextMenu" && !(e.shiftKey && e.key === "F10")) return;
      e.preventDefault();
      const box = e.currentTarget.getBoundingClientRect();
      open(channel, programme, { x: box.left + 12, y: box.bottom }, true);
    },
  };
}
