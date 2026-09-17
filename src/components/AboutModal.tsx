// About. Small on purpose: what this is, what it is talking to, and the
// version — nothing that needs maintaining.

import { useApp, useDispatch } from "../store";

export function AboutModal() {
  const { sync } = useApp();
  const dispatch = useDispatch();
  const close = () => dispatch({ type: "about", open: false });

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) close();
      }}
    >
      <div className="about" role="dialog" aria-label="About zapper">
        <h2>zapper</h2>
        <p className="about-tag">An IPTV player that treats 50,000 channels as normal.</p>
        <dl className="about-facts">
          <div>
            <dt>Catalogue</dt>
            <dd>
              {sync.channelsTotal > 0
                ? `${sync.channelsTotal.toLocaleString()} channels`
                : "not synced yet"}
            </dd>
          </div>
          <div>
            <dt>Playback</dt>
            <dd>mpv, drawn beneath the window</dd>
          </div>
        </dl>
        <div className="about-actions">
          <button className="ctl" onClick={close} autoFocus>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
