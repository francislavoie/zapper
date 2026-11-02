// A browser's picture, and what the page says beside it.
//
// The <video> element is where mpv's picture would be in the app: beneath
// the interface, placed by the same measurements (useVideoRect paints it
// into the hole the band or the player leaves). src/webPlayer.ts plays into
// it.
//
// Beside it, two things the service tells this page: a recording on another
// channel that needs the connection being watched on (the account has no
// other; a viewer is never cut off for it, so it waits — docs/recording.md),
// with the two ways out, and a channel whose sound is left out.

import { useEffect, useSyncExternalStore, type MutableRefObject } from "react";
import { backend } from "../backend";
import { playChannel } from "../actions";
import { hhmm } from "../logic/format";
import { noticeText } from "../logic/webPlayback";
import { run, useDispatch } from "../store";
import { attachVideo, noticeFeed, type WebNotice } from "../webPlayer";

export interface WebVideoProps {
  /** Where useVideoRect paints the picture's place. */
  planeRef: MutableRefObject<HTMLElement | null>;
}

export function WebVideo({ planeRef }: WebVideoProps) {
  useEffect(() => () => attachVideo(null), []);
  return (
    <>
      <video
        className="web-video"
        ref={(el) => {
          planeRef.current = el;
          attachVideo(el);
        }}
        playsInline
        aria-hidden="true"
        tabIndex={-1}
      />
      <WebNoticeCard />
    </>
  );
}

const subscribe = (f: () => void) => noticeFeed.on(f);
const current = (): WebNotice => noticeFeed.last;

function WebNoticeCard() {
  const notice = useSyncExternalStore(subscribe, current);
  const dispatch = useDispatch();
  const rec = notice.recording;
  if (!rec && !notice.silent) return null;

  const watchThat = async () => {
    const [ch] = await backend.channelsByIds([rec!.streamId]);
    if (ch) void playChannel(dispatch, ch);
  };

  return (
    <div className="web-notice" role="status">
      {rec ? (
        <>
          <p>{noticeText(rec, (t) => hhmm(t))}</p>
          <div className="web-notice-actions">
            <button className="ctl" onClick={() => void watchThat()}>
              Watch {rec.channel}
            </button>
            <button className="ctl" onClick={() => void run(dispatch, "stop", backend.stop())}>
              Stop watching
            </button>
          </div>
          <p className="web-notice-note">
            Keep watching and it waits for you; close this page and it starts.
          </p>
        </>
      ) : (
        <p>{notice.silent}</p>
      )}
    </div>
  );
}
