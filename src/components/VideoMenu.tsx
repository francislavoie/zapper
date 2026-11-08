// The right-click menu on the picture, full size or docked.
//
// WebKit's own context menu is suppressed at the GTK level — Back, Reload
// and Inspect Element mean nothing here, and "Reload" would throw away the
// running player. This is what takes its place over the video: things that
// are about the stream, and only those that apply to it. A single audio
// track is no choice, so there is no audio row; no subtitles, no subtitle
// row (except on a movie or an episode, where it is the way to find some
// online: components/FindSubtitles.tsx); nothing playing, none of the
// picture's settings at all.
//
// The choices sit in submenus, each row saying what is chosen now, so the
// menu stays a short list over a picture it should not hide. Below them are
// the three views of the picture, with their keys. Above them, Record: the
// programme on now (components/PlayerRecord.tsx).
//
// The same menu narrowed to one submenu (`only`) is what the transport bar's
// audio button, sleep moon and (on a title with no subtitles to show)
// subtitles button open: the same choices, from a control.

import { runAction, togglePictureFullscreen } from "../actions";
import { backend, capabilities } from "../backend";
import { useFullscreen } from "../hooks/useFullscreen";
import {
  ASPECT_CHOICES,
  DEINTERLACE_CHOICES,
  choiceLabel,
  hasAudioChoice,
  isPictureSubtitle,
  PICTURE_SUBTITLES_NOTE,
  selectedLabel,
  trackLabel,
} from "../logic/tracks";
import { run, useApp, useDispatch, type VideoMenu as VideoMenuState } from "../store";
import type { PlayerOptionsDto } from "../types";
import { ContextMenu } from "./ContextMenu";
import { usePlayerRecord } from "../hooks/usePlayerRecord";
import { PlayerRecordItems } from "./PlayerRecord";
import { useSleepLeft } from "../hooks/useSleep";
import { useOnDemandTitle } from "../hooks/useOnDemandTitle";
import { useSubtitles } from "../hooks/useSubtitles";
import { shownTrack } from "../logic/subtitles";
import { SleepItems, SleepSubMenu } from "./SleepMenu";
import { OnlineSubtitleItems } from "./FindSubtitles";
import { CheckItem, RadioItem, SubMenu } from "./SubMenu";

export interface VideoMenuProps {
  menu: VideoMenuState;
  onClose: () => void;
}

export function VideoMenu({ menu, onClose }: VideoMenuProps) {
  const state = useApp();
  const dispatch = useDispatch();
  const fullscreen = useFullscreen();
  const sleepLeft = useSleepLeft();
  const { options, player } = state;
  const playing = player.state !== "stopped";
  const record = usePlayerRecord();
  const shown = shownTrack(options);
  const onDemand = useOnDemandTitle() !== null;

  /** Every choice closes the menu: it has been answered. The new state comes
   *  back from the command and is applied at once, rather than a heartbeat
   *  later when the backend's push arrives. */
  const choose = (what: string, p: Promise<PlayerOptionsDto>) => {
    onClose();
    void run(
      dispatch,
      what,
      p.then((value) => dispatch({ type: "options", value })),
    );
  };

  const anchor = menu.anchor ? { current: menu.anchor } : undefined;

  if (menu.only === "audio") {
    return (
      <ContextMenu x={menu.x} y={menu.y} heading="Audio track" onClose={onClose}
                   takeFocus={menu.keyboard} anchor={anchor} above={menu.above}>
        <AudioItems options={options} choose={choose} />
      </ContextMenu>
    );
  }

  if (menu.only === "subtitles") {
    return (
      <ContextMenu x={menu.x} y={menu.y} heading="Subtitles" onClose={onClose}
                   takeFocus={menu.keyboard} anchor={anchor} above={menu.above}>
        <SubtitleItems options={options} shown={shown} choose={choose} onClose={onClose} />
      </ContextMenu>
    );
  }

  if (menu.only === "sleep") {
    return (
      <ContextMenu
        x={menu.x}
        y={menu.y}
        heading={sleepLeft ? `Sleep timer · ${sleepLeft} left` : "Sleep timer"}
        onClose={onClose}
        takeFocus={menu.keyboard}
        anchor={anchor}
        above={menu.above}
      >
        <SleepItems />
      </ContextMenu>
    );
  }

  return (
    <ContextMenu
      x={menu.x}
      y={menu.y}
      heading={state.nowChannel?.cleanName ?? "Picture"}
      onClose={onClose}
      takeFocus={menu.keyboard}
    >
      {record ? (
        <>
          <PlayerRecordItems record={record} onClose={onClose} />
          <div className="ctx-sep" role="separator" />
        </>
      ) : null}

      {hasAudioChoice(options) ? (
        <SubMenu label="Audio track" value={selectedLabel(options.audio) ?? undefined}>
          <AudioItems options={options} choose={choose} />
        </SubMenu>
      ) : null}

      {options.subtitles.length > 0 || onDemand ? (
        // Off is a hidden track (it stays chosen, so showing it again is not
        // a switch): only the track on screen is ticked.
        <SubMenu
          label="Subtitles"
          value={
            options.subtitles.length > 0
              ? shown
                ? (selectedLabel(options.subtitles) ?? "Off")
                : "Off"
              : undefined
          }
        >
          <SubtitleItems options={options} shown={shown} choose={choose} onClose={onClose} />
        </SubMenu>
      ) : null}

      {playing ? (
        <>
          <SubMenu label="Aspect ratio" value={choiceLabel(ASPECT_CHOICES, options.aspect)}>
            {ASPECT_CHOICES.map((c) => (
              <RadioItem
                key={c.value}
                checked={options.aspect === c.value}
                label={c.label}
                onSelect={() => choose("aspect ratio", backend.setAspect(c.value))}
              />
            ))}
          </SubMenu>
          {/* mpv's; a browser deinterlaces by itself. */}
          {capabilities.mpv ? (
            <SubMenu
              label="Deinterlace"
              value={choiceLabel(DEINTERLACE_CHOICES, options.deinterlace)}
            >
              {DEINTERLACE_CHOICES.map((c) => (
                <RadioItem
                  key={c.value}
                  checked={options.deinterlace === c.value}
                  label={c.label}
                  onSelect={() => choose("deinterlace", backend.setDeinterlace(c.value))}
                />
              ))}
            </SubMenu>
          ) : null}
          <SleepSubMenu />
          <div className="ctx-sep" role="separator" />
        </>
      ) : null}

      {capabilities.mpv ? (
        <CheckItem
          checked={state.statsOpen}
          label="Stats for nerds"
          hint="S"
          onSelect={() => dispatch({ type: "stats", open: !state.statsOpen })}
        />
      ) : null}
      {playing ? (
        <CheckItem
          checked={state.mode === "watch"}
          label="Theater"
          hint="T"
          onSelect={() => {
            onClose();
            if (state.mode === "watch") dispatch({ type: "mode", mode: "browse" });
            else runAction("theater", { dispatch, player, previous: state.previousChannel });
          }}
        />
      ) : null}
      <CheckItem
        checked={fullscreen}
        label="Fullscreen"
        hint="F"
        onSelect={() => {
          onClose();
          togglePictureFullscreen(dispatch, fullscreen ? "watch" : state.mode);
        }}
      />
    </ContextMenu>
  );
}

/** The stream's subtitle tracks, then the ones from OpenSubtitles for a
 *  movie or an episode. "Off" only when there is a track to turn off. */
function SubtitleItems({
  options,
  shown,
  choose,
  onClose,
}: {
  options: PlayerOptionsDto;
  shown: ReturnType<typeof shownTrack>;
  choose: (what: string, p: Promise<PlayerOptionsDto>) => void;
  onClose: () => void;
}) {
  // A choice here is the standing one: every open after shows the same.
  const subtitles = useSubtitles();
  return (
    <>
      {options.subtitles.length > 0 ? (
        <RadioItem
          checked={!shown}
          label="Off"
          onSelect={() => choose("subtitles", subtitles.choose(null))}
        />
      ) : null}
      {options.subtitles.map((t, i) => {
        const l = trackLabel(t, i);
        return (
          <RadioItem
            key={t.id}
            checked={shown?.id === t.id}
            label={l.label}
            detail={l.detail}
            disabled={!capabilities.pictureSubtitles && isPictureSubtitle(t)}
            onSelect={() => choose("subtitles", subtitles.choose(t.id))}
          />
        );
      })}
      {!capabilities.pictureSubtitles && options.subtitles.some(isPictureSubtitle) ? (
        <p className="ctx-note">{PICTURE_SUBTITLES_NOTE}</p>
      ) : null}
      <OnlineSubtitleItems onClose={onClose} />
    </>
  );
}

function AudioItems({
  options,
  choose,
}: {
  options: PlayerOptionsDto;
  choose: (what: string, p: Promise<PlayerOptionsDto>) => void;
}) {
  return (
    <>
      {options.audio.map((t, i) => {
        const l = trackLabel(t, i);
        return (
          <RadioItem
            key={t.id}
            checked={t.selected}
            label={l.label}
            detail={l.detail}
            onSelect={() => choose("audio track", backend.setAudioTrack(t.id))}
          />
        );
      })}
    </>
  );
}
