// What zapper is, which version this is, its license, and what it is built on, with the way in
// to every license notice the packages ship (docs/licensing.md). Settings > About shows it, and
// so does the About dialog the palette opens: one place to keep current rather than two.
//
// Checking for updates is the Updates pane's; this only says which version is running.

import { lazy, Suspense, useState } from "react";
import { useApp } from "../store";

// The licenses view loads on first use: none of it is in the startup bundle.
const Licenses = lazy(() => import("./Licenses"));

export function AboutPane({ onLicenses }: { onLicenses?: (open: boolean) => void }) {
  const { update } = useApp();
  const [licenses, setLicenses] = useState(false);
  const show = (open: boolean) => {
    setLicenses(open);
    onLicenses?.(open);
  };

  if (licenses) {
    return (
      <div className="about-licenses">
        <div className="about-licenses-head">
          <button className="ctl" onClick={() => show(false)} autoFocus>
            Back
          </button>
          <h4>Open source licenses</h4>
        </div>
        <Suspense fallback={<p className="licenses-missing" aria-busy="true">Loading…</p>}>
          <Licenses />
        </Suspense>
      </div>
    );
  }

  return (
    <div className="about-pane">
      <div className="about-id">
        <span className="about-name">zapper</span>
        {/* The backend's own version, as the updater compares it; empty until its first status
            arrives, and then there is nothing worth saying in its place. */}
        {update.current ? <span className="about-version">version {update.current}</span> : null}
      </div>
      <p>
        A live-TV player for IPTV: Xtream Codes servers, M3U playlists, HDHomeRun and Tvheadend.
      </p>
      <p className="about-license">MIT License · © 2026 Francis Lavoie</p>
      <p>
        Video is played by mpv and decoded by FFmpeg, free software under the GNU LGPL and GPL.
        zapper is built with Tauri, React and the other open source projects whose notices are
        listed here.
      </p>
      <div className="about-pane-actions">
        <button className="ctl" onClick={() => show(true)}>
          Open source licenses
        </button>
      </div>
    </div>
  );
}
