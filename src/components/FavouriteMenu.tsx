// The context menu over a favourite or a recently watched channel: forget it.
//
// The surface, its placement and how it closes are ContextMenu's; this is
// only what it says.

import type { FavouriteMenu as FavouriteMenuState } from "../store";
import { ContextMenu } from "./ContextMenu";

export interface FavouriteMenuProps {
  menu: FavouriteMenuState;
  onForget: () => void;
  onClose: () => void;
}

export function FavouriteMenu({ menu, onForget, onClose }: FavouriteMenuProps) {
  return (
    <ContextMenu x={menu.x} y={menu.y} heading={menu.name} onClose={onClose}>
      <button className="tb-menu-item" role="menuitem" onClick={onForget}>
        <span className="tb-menu-label" data-tip-overflow="">
          Forget this channel
        </span>
      </button>
      {menu.meta ? <div className="tb-menu-foot">{menu.meta}</div> : null}
      <div className="tb-menu-foot">
        Clears its watch history, so it stops crowding the list.
      </div>
    </ContextMenu>
  );
}
