// The desktop's media controls: what they are told, what each key does, and
// which channel next and previous land on.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  indexOfStream,
  intentOf,
  mediaSessionOf,
  movieSessionOf,
  playbackOf,
  sessionKey,
  stepFrom,
} from "../logic/mediaKeys.ts";
import { DEFAULT_SETTINGS, clampSettings } from "../logic/settings.ts";

const channel = (streamId, p = {}) => ({
  streamId,
  name: `|UK| Channel ${streamId} HD`,
  cleanName: `Channel ${streamId}`,
  num: streamId,
  logoUrl: `http://img.example/${streamId}.png`,
  categoryId: "c",
  hasArchive: false,
  archiveDays: null,
  isSeparator: false,
  quality: null,
  nowPlaying: null,
  ...p,
});
const separator = (streamId) => channel(streamId, { isSeparator: true, name: "###### X ######" });

test("media controls are off unless turned on, an old config included", () => {
  assert.equal(DEFAULT_SETTINGS.mediaControls, false);
  const old = { ...DEFAULT_SETTINGS };
  delete old.mediaControls;
  assert.equal(clampSettings(old).mediaControls, false);
  assert.equal(clampSettings({ ...DEFAULT_SETTINGS, mediaControls: "yes" }).mediaControls, false);
  assert.equal(clampSettings({ ...DEFAULT_SETTINGS, mediaControls: true }).mediaControls, true);
});

test("a movie's session is its title, with its year and length beneath", () => {
  const m = { name: "Some Film (2025)", cleanName: "Some Film", year: 2025, durationSecs: 6540 };
  assert.deepEqual(movieSessionOf(m, "paused"), {
    title: "Some Film",
    subtitle: "2025 · 1h 49m",
    artUrl: null,
    playback: "paused",
  });
  assert.equal(movieSessionOf(m, "stopped"), null);
  assert.equal(movieSessionOf(null, "playing"), null);
});

test("the session is the channel, its programme and its logo", () => {
  const s = mediaSessionOf(
    channel(7),
    { start: 0, stop: 1, title: " The News ", description: null },
    "playing",
  );
  assert.deepEqual(s, {
    title: "Channel 7",
    subtitle: "The News",
    artUrl: "http://img.example/7.png",
    playback: "playing",
  });
  assert.equal(mediaSessionOf(channel(7, { logoUrl: null }), null, "paused").subtitle, null);
  assert.equal(mediaSessionOf(channel(7, { logoUrl: "" }), null, "paused").artUrl, null);
  assert.equal(mediaSessionOf(channel(7, { cleanName: "" }), null, "paused").title, "|UK| Channel 7 HD");
});

test("nothing on is no session, and the state reads as the desktop expects", () => {
  assert.equal(mediaSessionOf(null, null, "playing"), null);
  assert.equal(mediaSessionOf(channel(1), null, "stopped"), null);
  assert.equal(playbackOf("opening"), "playing");
  assert.equal(playbackOf("paused"), "paused");
  assert.equal(playbackOf("failed"), "stopped");
});

test("the session key changes exactly when the session does", () => {
  const a = mediaSessionOf(channel(1), null, "playing");
  assert.equal(sessionKey(a), sessionKey({ ...a }));
  assert.notEqual(sessionKey(a), sessionKey({ ...a, playback: "paused" }));
  assert.notEqual(sessionKey(a), sessionKey({ ...a, subtitle: "x" }));
  assert.equal(sessionKey(null), "");
});

test("play and pause are not toggles; next and previous always ask", () => {
  assert.equal(intentOf("toggle", "playing"), "togglePause");
  assert.equal(intentOf("toggle", "paused"), "togglePause");
  assert.equal(intentOf("toggle", "stopped"), null);
  assert.equal(intentOf("play", "paused"), "togglePause");
  assert.equal(intentOf("play", "playing"), null, "play must not pause");
  assert.equal(intentOf("pause", "playing"), "togglePause");
  assert.equal(intentOf("pause", "paused"), null, "pause must not resume");
  assert.equal(intentOf("pause", "opening"), "togglePause");
  assert.equal(intentOf("next", "stopped"), "next");
  assert.equal(intentOf("previous", "paused"), "previous");
  assert.equal(intentOf("stop", "playing"), "stop");
  assert.equal(intentOf("stop", "stopped"), null);
});

/** A paged list: `rows` with gaps (undefined) where a page is not loaded. */
const list = (rows) => (i) => rows[i];

test("the playing channel is found among the loaded rows", () => {
  const rows = [channel(10), undefined, channel(12), channel(13)];
  assert.equal(indexOfStream(list(rows), rows.length, 12), 2);
  assert.equal(indexOfStream(list(rows), rows.length, 99), -1);
});

test("next and previous skip separators and stop at the ends", () => {
  const rows = [channel(1), separator(2), channel(3), channel(4)];
  const at = (from, dir) => stepFrom(list(rows), rows.length, from, dir);
  assert.deepEqual(at(0, 1), { kind: "found", index: 2, channel: rows[2] });
  assert.deepEqual(at(2, -1), { kind: "found", index: 0, channel: rows[0] });
  assert.deepEqual(at(3, 1), { kind: "end" }, "no wrapping to the top");
  assert.deepEqual(at(0, -1), { kind: "end" });
  // A list of separators past the playing row ends too.
  assert.deepEqual(stepFrom(list([channel(1), separator(2)]), 2, 0, 1), { kind: "end" });
});

test("a row not loaded yet is asked for rather than skipped", () => {
  const rows = [channel(1), separator(2), undefined, channel(4)];
  assert.deepEqual(stepFrom(list(rows), rows.length, 0, 1), { kind: "missing", index: 2 });
  assert.deepEqual(stepFrom(list(rows), rows.length, 3, -1), { kind: "missing", index: 2 });
});
