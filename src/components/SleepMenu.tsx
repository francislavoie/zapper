// The sleep timer's choices, as menu items. One list for every place they
// appear — the picture's right-click menu, the hamburger, and the moon in the
// transport bar — and built from the command palette's own sleep actions, so
// what can be chosen, and when, is decided once (logic/palette.ts).

import { runAction } from "../actions";
import { useSleepActions, useSleepLeft } from "../hooks/useSleep";
import { hhmm } from "../logic/format";
import type { PaletteAction } from "../logic/palette";
import { isRequest } from "../logic/sleep";
import { useApp, useDispatch } from "../store";
import { RadioItem, SubMenu } from "./SubMenu";

/** The choices, ticked where the timer stands. "Off" is always there and is
 *  ticked when no timer is set: it is one of the choices, not a command. */
export function SleepItems() {
  const state = useApp();
  const dispatch = useDispatch();
  const actions = useSleepActions();
  const ctx = { dispatch, player: state.player, previous: state.previousChannel };
  const choose = (a: PaletteAction) => runAction(a.id, ctx);

  return (
    <>
      {actions
        .filter((a) => a.sleep != null)
        .map((a) => (
          <RadioItem
            key={a.id}
            checked={a.sleep != null && isRequest(state.sleep, a.sleep)}
            label={a.short ?? a.label}
            detail={
              a.sleep === "programme" && state.sleepProgramme
                ? `until ${hhmm(state.sleepProgramme.stop)}`
                : undefined
            }
            onSelect={() => choose(a)}
          />
        ))}
      <RadioItem
        checked={state.sleep === null}
        label="Off"
        onSelect={() => runAction("sleepOff", ctx)}
      />
    </>
  );
}

/** "Sleep timer ›", saying how long is left. Nothing while nothing plays. */
export function SleepSubMenu() {
  const actions = useSleepActions();
  const left = useSleepLeft();
  if (actions.length === 0) return null;
  return (
    <SubMenu label="Sleep timer" value={left ? `${left} left` : "Off"}>
      <SleepItems />
    </SubMenu>
  );
}
