// Backend rejections land here rather than in the console. Every command can
// fail and today every command does fail, so this is the honest channel for
// "that did not work" without blanking a pane that still has good data in it.

import { useEffect } from "react";
import { useApp, useDispatch } from "../store";

export function Toasts() {
  const { toasts } = useApp();
  const dispatch = useDispatch();

  useEffect(() => {
    if (toasts.length === 0) return;
    const oldest = toasts[0];
    const t = window.setTimeout(
      () => dispatch({ type: "dismissToast", id: oldest.id }),
      6000,
    );
    return () => window.clearTimeout(t);
  }, [toasts, dispatch]);

  if (toasts.length === 0) return null;
  return (
    <div className="toasts">
      {toasts.map((t) => (
        <button
          key={t.id}
          className={`toast ${t.kind}`}
          onClick={() => dispatch({ type: "dismissToast", id: t.id })}
          title="dismiss"
        >
          {t.text}
        </button>
      ))}
    </div>
  );
}
