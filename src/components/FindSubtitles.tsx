// "Find subtitles…": subtitles from OpenSubtitles for the movie or episode
// that is open, from the Subtitles part of the picture's menu.
//
// The menu's part (`OnlineSubtitleItems`) offers what was downloaded before
// for this title and isn't loaded yet, then the way into the dialog. The
// dialog (`FindSubtitles`) searches in the languages Settings › Online
// services names (or the standing subtitle language and the interface's),
// lists what was downloaded before and then what was found, and adds the
// file chosen (its row's Download button, Enter, or a double-click; never a
// single click, which only highlights) to the player at once. A first
// download asks for the OpenSubtitles sign-in in place of the list, and runs
// once more after it.
// A refusal (no downloads left, a service asking to wait, the service off)
// is said in place, with the list kept.

import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { backend } from "../backend";
import { useFocusTrap } from "../hooks/useFocusTrap";
import { useOnDemandTitle } from "../hooks/useOnDemandTitle";
import { useSubtitles } from "../hooks/useSubtitles";
import { errorMessage, failed, loading, ready, type Async } from "../logic/async";
import {
  downloadNotice,
  downloadTrackTitle,
  findHeading,
  keptTrack,
  offeredDownloads,
  resultLine,
  searchLanguages,
  searchNotice,
  signInPrompt,
  trackLanguage,
} from "../logic/onlineSubtitles";
import { languageName } from "../logic/tracks";
import {
  closeFindSubtitles,
  downloadAnswered,
  findOpener,
  keptChanged,
  openFindSubtitles,
  useFindSubtitles,
  useKeptSubtitles,
  useOnlineServices,
  type TitleRef,
} from "../onlineSubtitles";
import { run, useApp, useDispatch } from "../store";
import type { KeptSubtitleDto, SubtitleResultDto, SubtitleSearchDto } from "../types";
import { OpenSubtitlesSignIn } from "./OpenSubtitlesSignIn";
import { RadioItem } from "./SubMenu";

const same = (a: TitleRef | null, b: TitleRef | null) =>
  a !== null && b !== null && a.kind === b.kind && a.id === b.id;

/** Adds a kept subtitle to what is playing; it comes back selected. */
function useAddKept() {
  const { settings } = useApp();
  const dispatch = useDispatch();
  return async (t: TitleRef, k: KeptSubtitleDto) => {
    const value = await backend.addSubtitleFile(
      t.kind,
      t.id,
      k.fileId,
      downloadTrackTitle(k.release),
      trackLanguage(k.language, settings.subtitleLanguage),
    );
    dispatch({ type: "options", value });
  };
}

/** In the Subtitles menu, after the stream's own tracks. `opener` is the
 *  control the menu was opened from, which focus goes back to after the
 *  dialog. */
export function OnlineSubtitleItems({
  onClose,
  opener,
}: {
  onClose: () => void;
  opener?: HTMLElement | null;
}) {
  const { options } = useApp();
  const dispatch = useDispatch();
  const title = useOnDemandTitle();
  const kept = useKeptSubtitles(title);
  const addKept = useAddKept();
  if (!title) return null;
  return (
    <>
      {/* Hidden when it would be first (styles.css): no tracks above. */}
      <div className="ctx-sep" role="separator" />
      {offeredDownloads(kept, options.subtitles).map((k) => (
        <RadioItem
          key={k.fileId}
          checked={false}
          label={languageName(k.language) ?? k.language}
          detail={downloadTrackTitle(k.release)}
          onSelect={() => {
            onClose();
            void run(dispatch, "subtitles", addKept(title, k));
          }}
        />
      ))}
      <button
        className="tb-menu-item"
        role="menuitem"
        onClick={() => {
          onClose();
          openFindSubtitles(title, opener);
        }}
      >
        <span className="ctx-check" />
        <span className="tb-menu-label">Find subtitles…</span>
      </button>
    </>
  );
}

export function FindSubtitles() {
  const title = useFindSubtitles();
  if (!title) return null;
  return <FindDialog key={`${title.kind}:${title.id}`} target={title} />;
}

type Row = { kept: KeptSubtitleDto } | { result: SubtitleResultDto };

function languages(codes: string[]): string {
  const names = codes.map((c) => languageName(c) ?? c);
  return names.length < 2 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;
}

const now = () => Math.floor(Date.now() / 1000);

/** The player's own control to give focus back to: its subtitles button,
 *  else the first of its transport controls (play/pause). */
function playerControl(): HTMLElement | null {
  return (
    document.querySelector<HTMLElement>('.ctl-row button[aria-label="Subtitles"]') ??
    document.querySelector<HTMLElement>(".ctl-row button")
  );
}

function FindDialog({ target: title }: { target: TitleRef }) {
  const app = useApp();
  const dispatch = useDispatch();
  const services = useOnlineServices();
  const kept = useKeptSubtitles(title);
  const subtitles = useSubtitles();
  const addKept = useAddKept();
  const playing = useOnDemandTitle();
  const listId = useId();

  // A title that stops or changes under the dialog takes it with it.
  const wasPlaying = useRef(same(playing, title));
  useEffect(() => {
    if (wasPlaying.current && !same(playing, title)) closeFindSubtitles();
  }, [playing, title]);

  const movie = title.kind === "movie" && app.movies.playing?.movieId === title.id ? app.movies.playing : null;
  const episode =
    title.kind === "episode" && app.shows.playing?.episode.episodeId === title.id ? app.shows.playing : null;
  const heading = findHeading(movie, episode) || "Find subtitles";

  const settled = services.status.status === "ready" || services.status.status === "error";
  const configured = services.status.status === "ready" ? services.status.data.openSubtitles.languages : [];
  const searched = useMemo(
    () => searchLanguages(configured, app.settings.subtitleLanguage, navigator.languages ?? []),
    // By value: a new array with the same codes is the same search.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [configured.join(","), app.settings.subtitleLanguage],
  );

  const [search, setSearch] = useState<Async<SubtitleSearchDto>>(loading());
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    if (!settled) return;
    let current = true;
    setSearch(loading());
    backend.findSubtitles(title.kind, title.id, searched).then(
      (s) => current && setSearch(ready(s)),
      (e: unknown) => current && setSearch(failed(errorMessage(e))),
    );
    return () => {
      current = false;
    };
  }, [settled, searched, attempt, title]);

  const results = search.status === "ready" && search.data.kind === "results" ? search.data.results : [];
  const rows: Row[] = [...kept.map((k) => ({ kept: k })), ...results.map((r) => ({ result: r }))];
  const [active, setActive] = useState(0);
  const at = Math.min(active, Math.max(rows.length - 1, 0));

  const [signIn, setSignIn] = useState<{ fileId: number; refused: boolean; email: boolean } | null>(
    null,
  );
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState<number | null>(null);
  const busyRef = useRef(false);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  /** The subtitle is kept: into the player, if this title is what plays. */
  const added = async (k: KeptSubtitleDto) => {
    if (same(playing, title)) {
      const track = keptTrack(k, app.options.subtitles);
      if (track) {
        dispatch({ type: "options", value: await subtitles.choose(track.id) });
        closeFindSubtitles();
        return;
      }
      await addKept(title, k);
      closeFindSubtitles();
      dispatch({ type: "toast", text: "Subtitles added", kind: "info" });
    } else {
      closeFindSubtitles();
      dispatch({ type: "toast", text: "Subtitles downloaded", kind: "info" });
    }
  };

  /** One at a time: a double-click must not become two downloads. */
  const act = async (what: () => Promise<void>, fileId: number) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(fileId);
    setNotice(null);
    try {
      await what();
    } catch (e) {
      if (alive.current) setNotice(errorMessage(e));
    } finally {
      busyRef.current = false;
      if (alive.current) setBusy(null);
    }
  };

  const download = (fileId: number) =>
    act(async () => {
      const d = await backend.downloadSubtitle(title.kind, title.id, fileId);
      // Kept, and counted, even when the dialog was closed meanwhile.
      downloadAnswered(d);
      if (d.kind === "downloaded") keptChanged(title);
      if (!alive.current) return;
      if (d.kind === "downloaded") {
        await added(d.subtitle);
      } else if (d.kind === "signIn") {
        setSignIn({ fileId, refused: d.refused, email: d.email });
      } else {
        setNotice(downloadNotice(d, now()));
      }
    }, fileId);

  const choose = (row: Row | undefined) => {
    if (!row) return;
    const k = "kept" in row ? row.kept : kept.find((x) => x.fileId === row.result.fileId);
    if (k) void act(() => added(k), k.fileId);
    else if ("result" in row) void download(row.result.fileId);
  };

  // Escape and the mouse's back button close this as the top step of the
  // app's one chain (logic/escape.ts, hooks/useGlobalKeys.ts).

  // Focus comes in, stays in (Tab wraps), and goes back when it closes: to
  // the control it was opened from, or (that gone, as a menu item is) to
  // the player's controls. Never to nothing, where the keys would be lost.
  const boxRef = useRef<HTMLDivElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const trap = useFocusTrap(boxRef);
  const opener = useRef(findOpener());
  useEffect(() => {
    boxRef.current?.focus();
    const back = opener.current;
    return () => {
      const target =
        back && back.isConnected && back !== document.body ? back : playerControl();
      target?.focus();
    };
  }, []);
  const hasRows = rows.length > 0;
  const signingIn = signIn !== null;
  useEffect(() => {
    if (signingIn) {
      boxRef.current?.querySelector<HTMLInputElement>("input")?.focus();
      return;
    }
    const focus = document.activeElement;
    // Back from the sign-in, or the first rows in: the list takes the keys.
    if (hasRows && (focus === boxRef.current || focus === document.body || focus === null)) {
      listRef.current?.focus();
    }
  }, [hasRows, signingIn]);

  useEffect(() => {
    document.getElementById(`${listId}-${at}`)?.scrollIntoView?.({ block: "nearest" });
  }, [at, listId]);

  const onListKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = e.key === "ArrowDown" ? 1 : e.key === "ArrowUp" ? -1 : 0;
    // Moving from a row's Download button: the list takes the keys again,
    // rather than focus staying on a button that is hidden once its row
    // isn't the highlighted one.
    const moving = step !== 0 || e.key === "Home" || e.key === "End";
    if (moving && e.target !== listRef.current) listRef.current?.focus();
    if (step) {
      e.preventDefault();
      setActive(Math.max(0, Math.min(rows.length - 1, at + step)));
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      setActive(e.key === "Home" ? 0 : rows.length - 1);
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(rows[at]);
    }
  };

  const option = (row: Row, i: number) => {
    const k = "kept" in row ? row.kept : null;
    const line = k
      ? {
          label: languageName(k.language) ?? k.language,
          detail: [k.release.trim(), k.hearingImpaired ? "SDH" : ""].filter(Boolean).join(" · "),
        }
      : resultLine((row as { result: SubtitleResultDto }).result);
    const release = (k ? k.release : (row as { result: SubtitleResultDto }).result.release).trim();
    const rest = line.detail.slice(release.length).replace(/^ · /, "");
    const fileId = k ? k.fileId : (row as { result: SubtitleResultDto }).result.fileId;
    // Already here: adding it spends nothing.
    const have = k !== null || kept.some((x) => x.fileId === fileId);
    return (
      <div
        key={`${k ? "k" : "r"}${fileId}`}
        id={`${listId}-${i}`}
        role="option"
        aria-selected={i === at}
        className={`find-subs-row ${i === at ? "active" : ""}`}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => {
          setActive(i);
          listRef.current?.focus();
        }}
        onDoubleClick={() => choose(row)}
      >
        <span className="find-subs-lang">{line.label}</span>
        <span className="find-subs-release" data-tip-overflow="">
          {release}
        </span>
        <span className="find-subs-meta">{busy === fileId ? "Downloading…" : rest}</span>
        {/* Shown on the row highlighted and the one under the pointer, and
            on every row on a touch screen (styles.css). A click on the row
            only highlights it: a stray one must not spend a download. */}
        <button
          className="ctl find-subs-get"
          tabIndex={i === at ? 0 : -1}
          aria-disabled={busy !== null || undefined}
          onClick={(e) => {
            e.stopPropagation();
            setActive(i);
            choose(row);
          }}
          onDoubleClick={(e) => e.stopPropagation()}
        >
          {have ? "Add" : "Download"}
        </button>
      </div>
    );
  };

  const searchLine = search.status === "ready" ? searchNotice(search.data, now()) : null;

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        // The mouse's side buttons never get here: useGlobalKeys takes them.
        if (e.target === e.currentTarget) closeFindSubtitles();
      }}
    >
      <div
        ref={boxRef}
        className="find-subs"
        role="dialog"
        aria-label="Find subtitles"
        aria-modal="true"
        tabIndex={-1}
        onKeyDown={(e) => {
          trap(e);
          // The player's keys (Space, the arrows) are not this dialog's.
          // Escape is the app's chain's, which closes this first.
          if (e.key !== "Escape" && !e.ctrlKey && !e.metaKey) e.stopPropagation();
        }}
      >
        <h2 className="find-subs-heading" data-tip-overflow="">
          {signIn ? "Sign in to download" : heading}
        </h2>

        {signIn ? (
          <div className="find-subs-signin">
            <p className="find-subs-line">{signInPrompt(signIn.refused, signIn.email)}</p>
            <OpenSubtitlesSignIn
              onSignedIn={(next) => {
                services.set(next);
                const { fileId } = signIn;
                setSignIn(null);
                // Once: a second refusal asks again rather than looping.
                void download(fileId);
              }}
            />
            <button className="ctl find-subs-back" onClick={() => setSignIn(null)}>
              Back
            </button>
          </div>
        ) : (
          <>
            <p className="find-subs-langs find-subs-line">
              Looking in {languages(searched)} ·{" "}
              <button
                className="find-subs-change"
                onClick={() => {
                  closeFindSubtitles();
                  dispatch({ type: "settings", open: true, page: "Online services" });
                }}
              >
                Change
              </button>
            </p>

            {notice ? (
              <p className="find-subs-notice" role="status">
                {notice}
              </p>
            ) : null}

            {hasRows ? (
              <div
                ref={listRef}
                className="find-subs-list"
                role="listbox"
                aria-label="Subtitles"
                aria-activedescendant={`${listId}-${at}`}
                aria-busy={busy !== null || undefined}
                tabIndex={0}
                onKeyDown={onListKey}
              >
                {kept.length ? (
                  <div role="group" aria-label="Downloaded before">
                    <div className="find-subs-group" aria-hidden="true">
                      Downloaded before
                    </div>
                    {kept.map((k, i) => option({ kept: k }, i))}
                  </div>
                ) : null}
                {results.length ? (
                  <div role="group" aria-label="Found">
                    {kept.length ? (
                      <div className="find-subs-group" aria-hidden="true">
                        Found
                      </div>
                    ) : null}
                    {results.map((r, i) => option({ result: r }, kept.length + i))}
                  </div>
                ) : null}
              </div>
            ) : null}

            {search.status === "loading" || search.status === "idle" ? (
              <p className="find-subs-state" role="status">
                Searching…
              </p>
            ) : search.status === "error" ? (
              <p className="find-subs-state bad" role="alert">
                {search.message}{" "}
                <button className="ctl" onClick={() => setAttempt((n) => n + 1)}>
                  Retry
                </button>
              </p>
            ) : searchLine ? (
              <p className="find-subs-state" role="status">
                {searchLine}
              </p>
            ) : null}
          </>
        )}

        <p className="find-subs-foot">Subtitles from OpenSubtitles.com</p>
      </div>
    </div>
  );
}
