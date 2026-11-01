// Account kinds: the capability comparison the settings pane shows, and the
// playlist form's keep-or-replace rules — which are the playlist's version of
// the write-only password, so getting them wrong either loses a saved
// address or sends `<redacted>` back as if it were one.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  SOURCE_KINDS,
  asKind,
  candidateFor,
  planXtreamSave,
  hdhrChanged,
  hdhrFormFrom,
  kindInfo,
  m3uChanged,
  m3uFormFrom,
  planHdhrSave,
  planM3uSave,
  planTvhSave,
  splitUrls,
  tvhChanged,
  tvhFormFrom,
} from "../logic/sources.ts";

const onFile = (p = {}) => ({
  playlist: "",
  playlistRedacted: false,
  epgUrls: [],
  epgRedacted: false,
  userAgent: "",
  ...p,
});

test("there are exactly the kinds the backend knows, each labelled once", () => {
  assert.deepEqual(
    SOURCE_KINDS.map((k) => k.id),
    ["xtream", "m3u", "hdhomerun", "tvheadend"],
  );
  const labels = SOURCE_KINDS.map((k) => k.label);
  assert.equal(new Set(labels).size, labels.length);
  for (const k of SOURCE_KINDS) assert.ok(k.needs.length > 0, k.id);
});

test("each card is a few short lines, not a paragraph", () => {
  for (const k of SOURCE_KINDS) {
    assert.ok(k.points.length >= 2 && k.points.length <= 4, `${k.id}: ${k.points.length} points`);
    // A card is half the pane wide; a point that wraps more than once there
    // has stopped being scannable.
    assert.ok(k.needs.length <= 45, `needs too long: ${k.needs}`);
    for (const p of k.points) assert.ok(p.text.length <= 50, `too long to scan: ${p.text}`);
  }
});

test("the cards are honest about what a playlist does not get", () => {
  // These are the differences the backend actually has; if one of them stops
  // being true, the point is what has to change.
  const m3u = kindInfo("m3u").points;
  const limit = (re) => m3u.find((p) => re.test(p.text));
  assert.equal(limit(/XMLTV/)?.ok, false, "guide needs an XMLTV link");
  assert.equal(limit(/catch-up/i)?.ok, false, "catch-up only where marked");
  assert.equal(limit(/status/i)?.ok, false, "no account status");
  assert.ok(kindInfo("xtream").points.every((p) => p.ok), "an Xtream server has no listed limits");
});

test("the tuner and server cards say what they cannot do", () => {
  const tuner = kindInfo("hdhomerun").points;
  const find = (pts, re) => pts.find((p) => re.test(p.text));
  assert.equal(find(tuner, /SiliconDust/)?.ok, true, "the free guide is the default");
  assert.equal(find(tuner, /catch-up/i)?.ok, false, "a tuner has no archive");
  assert.equal(find(tuner, /copy-protected/i)?.ok, false, "DRM channels are left out");
  const tvh = kindInfo("tvheadend").points;
  assert.equal(find(tvh, /persistent auth/i)?.ok, false, "the server setting is named up front");
  assert.equal(find(tvh, /recordings/i)?.ok, false, "no DVR");
});

test("an unknown or missing kind reads as Xtream", () => {
  assert.equal(asKind("m3u"), "m3u");
  assert.equal(asKind("hdhomerun"), "hdhomerun");
  assert.equal(asKind("tvheadend"), "tvheadend");
  assert.equal(asKind("xtream"), "xtream");
  assert.equal(asKind(undefined), "xtream");
  assert.equal(asKind("stalker"), "xtream");
  assert.equal(kindInfo("m3u").label, "M3U playlist");
});

test("guide addresses split on lines and on commas between addresses only", () => {
  assert.deepEqual(splitUrls("http://a/1.xml\n\n  http://a/2.xml.gz  \r\n"), [
    "http://a/1.xml",
    "http://a/2.xml.gz",
  ]);
  // x-tvg-url style.
  assert.deepEqual(splitUrls("http://a/1.xml,https://b/2.xml, /home/me/3.xml"), [
    "http://a/1.xml",
    "https://b/2.xml",
    "/home/me/3.xml",
  ]);
  // A comma inside a query string is part of the address.
  assert.deepEqual(splitUrls("http://a/epg.php?ids=1,2,3"), ["http://a/epg.php?ids=1,2,3"]);
  assert.deepEqual(splitUrls("http://a/x\nhttp://a/x"), ["http://a/x"]);
  assert.deepEqual(splitUrls("   "), []);
});

test("a redacted playlist starts empty and is kept unless retyped", () => {
  const a = onFile({
    playlist: "http://p/get.php?username=<redacted>&password=<redacted>",
    playlistRedacted: true,
  });
  const form = m3uFormFrom(a);
  assert.equal(form.playlist, "");
  assert.equal(m3uChanged(form, a), false);
  assert.deepEqual(planM3uSave(form, a), { playlist: null, epgUrls: [], userAgent: "" });

  const typed = { ...form, playlist: " http://new/list.m3u " };
  assert.equal(m3uChanged(typed, a), true);
  assert.equal(planM3uSave(typed, a).playlist, "http://new/list.m3u");
});

test("a plain playlist is shown as typed and sent back as typed", () => {
  const a = onFile({ playlist: "https://iptv-org.github.io/iptv/index.m3u" });
  const form = m3uFormFrom(a);
  assert.equal(form.playlist, a.playlist);
  assert.equal(planM3uSave(form, a).playlist, a.playlist);
});

test("with nothing on file a playlist is required", () => {
  const plan = planM3uSave(m3uFormFrom(onFile()), onFile());
  assert.ok("error" in plan);
});

test("a redacted guide list is kept until the field is typed in, then replaced", () => {
  const a = onFile({
    playlist: "/tv.m3u",
    epgUrls: ["http://p/xmltv.php?username=<redacted>"],
    epgRedacted: true,
  });
  const form = m3uFormFrom(a);
  assert.equal(form.epgText, "");
  assert.equal(planM3uSave(form, a).epgUrls, null);
  // Typing in it — even clearing it — is an answer.
  const cleared = { ...form, epgText: "", epgTouched: true };
  assert.deepEqual(planM3uSave(cleared, a).epgUrls, []);
  assert.equal(m3uChanged(cleared, a), false, "nothing typed is nothing changed");
  const replaced = { ...form, epgText: "http://g/new.xml", epgTouched: true };
  assert.deepEqual(planM3uSave(replaced, a).epgUrls, ["http://g/new.xml"]);
  assert.equal(m3uChanged(replaced, a), true);
});

test("a plain guide list is always sent, so clearing it clears it", () => {
  const a = onFile({ playlist: "/tv.m3u", epgUrls: ["http://g/a.xml"] });
  const form = m3uFormFrom(a);
  assert.equal(form.epgText, "http://g/a.xml");
  assert.deepEqual(planM3uSave(form, a).epgUrls, ["http://g/a.xml"]);
  const cleared = { ...form, epgText: "", epgTouched: true };
  assert.deepEqual(planM3uSave(cleared, a).epgUrls, []);
  assert.equal(m3uChanged(cleared, a), true);
});

const tunerOnFile = (p = {}) => ({
  hdhomerunDevice: "",
  hdhomerunEpgUrls: [],
  hdhomerunEpgRedacted: false,
  ...p,
});

test("a tuner needs an address, and an empty guide list means SiliconDust's", () => {
  assert.ok("error" in planHdhrSave(hdhrFormFrom(tunerOnFile()), tunerOnFile()));
  const a = tunerOnFile({ hdhomerunDevice: "192.168.1.50" });
  const form = hdhrFormFrom(a);
  assert.equal(hdhrChanged(form, a), false);
  assert.deepEqual(planHdhrSave(form, a), { device: "192.168.1.50", epgUrls: [] });
  const moved = { ...form, device: " 10.0.0.7 " };
  assert.equal(hdhrChanged(moved, a), true);
  assert.equal(planHdhrSave(moved, a).device, "10.0.0.7");
});

test("a tuner's redacted guide list is kept until typed in", () => {
  const a = tunerOnFile({
    hdhomerunDevice: "hdhr.local",
    hdhomerunEpgUrls: ["http://g/x.xml?key=<redacted>"],
    hdhomerunEpgRedacted: true,
  });
  const form = hdhrFormFrom(a);
  assert.equal(form.epgText, "");
  assert.equal(planHdhrSave(form, a).epgUrls, null);
  const cleared = { ...form, epgTouched: true };
  assert.deepEqual(planHdhrSave(cleared, a).epgUrls, []);
});

const serverOnFile = (p = {}) => ({
  tvheadendUrl: "",
  tvheadendUsername: "",
  tvheadendHasPassword: false,
  ...p,
});

test("a Tvheadend password is write-only, and a server may need no login", () => {
  const a = serverOnFile({
    tvheadendUrl: "http://tvh.lan:9981",
    tvheadendUsername: "viewer",
    tvheadendHasPassword: true,
  });
  const form = tvhFormFrom(a);
  assert.equal(form.password, "", "never filled in");
  assert.equal(tvhChanged(form, a), false);
  assert.deepEqual(planTvhSave(form, a), {
    url: "http://tvh.lan:9981",
    username: "viewer",
    password: "",
  });
  assert.equal(tvhChanged({ ...form, password: "new" }, a), true);
  // Anonymous: no login fields, and any typed password is not sent.
  assert.deepEqual(planTvhSave({ url: "tvh.lan:9981", username: "", password: "x" }, serverOnFile()), {
    url: "tvh.lan:9981",
    username: "",
    password: "",
  });
  // A username with no password typed or on file cannot log in.
  assert.ok("error" in planTvhSave({ url: "tvh.lan", username: "viewer", password: "" }, serverOnFile()));
  assert.ok("error" in planTvhSave({ url: " ", username: "", password: "" }, serverOnFile()));
});

test("the user agent is trimmed, and a changed one is a change", () => {
  const a = onFile({ playlist: "/tv.m3u", userAgent: "Kodi/21" });
  const form = m3uFormFrom(a);
  assert.equal(m3uChanged({ ...form, userAgent: " Kodi/21 " }, a), false);
  assert.equal(m3uChanged({ ...form, userAgent: "" }, a), true);
  assert.equal(planM3uSave({ ...form, userAgent: " TiviMate " }, a).userAgent, "TiviMate");
});

// ------------------------------------------------------- connection test

/** A whole account as the pane gets it: Xtream in use, a password on file. */
const account = (p = {}) => ({
  ...onFile(),
  host: "http://saved.example:8080",
  username: "saved",
  hasPassword: true,
  source: "zapper",
  path: "",
  kind: "xtream",
  hdhomerunDevice: "",
  hdhomerunEpgUrls: [],
  hdhomerunEpgRedacted: false,
  tvheadendUrl: "",
  tvheadendUsername: "",
  tvheadendHasPassword: false,
  ...p,
});

const forms = (p = {}) => ({
  xtream: { host: "", username: "", password: "" },
  m3u: { playlist: "", epgText: "", userAgent: "", epgTouched: false },
  hdhr: { device: "", epgText: "", epgTouched: false },
  tvh: { url: "", username: "", password: "" },
  ...p,
});

test("the test checks what is typed, not what is saved", () => {
  const typed = forms({
    xtream: { host: " http://new.example:8080 ", username: " me ", password: "typed" },
  });
  assert.deepEqual(candidateFor("xtream", typed, account()), {
    kind: "xtream",
    host: "http://new.example:8080",
    username: "me",
    password: "typed",
  });
});

test("a blank password is sent blank, to mean the one on file", () => {
  const typed = forms({ xtream: { host: "http://h", username: "me", password: "" } });
  assert.equal(candidateFor("xtream", typed, account()).password, "");
  // Unless there is none on file, which nothing could test.
  assert.ok("error" in candidateFor("xtream", typed, account({ hasPassword: false })));
});

test("the selected kind is tested, even when another is in use", () => {
  const typed = forms({ m3u: { playlist: "/tv.m3u", epgText: "", userAgent: "", epgTouched: false } });
  assert.deepEqual(candidateFor("m3u", typed, account()), {
    kind: "m3u",
    playlist: "/tv.m3u",
    epgUrls: [],
    userAgent: "",
  });
  const tuner = forms({ hdhr: { device: " 10.0.0.7 ", epgText: "", epgTouched: false } });
  assert.deepEqual(candidateFor("hdhomerun", tuner, account()), {
    kind: "hdhomerun",
    device: "10.0.0.7",
    epgUrls: [],
  });
  const server = forms({ tvh: { url: "http://tvh:9981", username: "", password: "" } });
  assert.deepEqual(candidateFor("tvheadend", server, account()), {
    kind: "tvheadend",
    url: "http://tvh:9981",
    username: "",
    password: "",
  });
});

test("a redacted playlist left alone is tested as the one on file", () => {
  const a = account({
    playlist: "http://p/get.php?username=<redacted>&password=<redacted>",
    playlistRedacted: true,
  });
  const c = candidateFor("m3u", forms({ m3u: m3uFormFrom(a) }), a);
  assert.equal(c.playlist, null);
});

test("an incomplete form says why rather than testing", () => {
  for (const kind of ["xtream", "m3u", "hdhomerun", "tvheadend"]) {
    const c = candidateFor(kind, forms(), account({ hasPassword: false }));
    assert.ok("error" in c, kind);
    assert.ok(c.error.length > 0, kind);
  }
  assert.deepEqual(planXtreamSave({ host: "h", username: " ", password: "x" }, { hasPassword: true }), {
    error: "a username is required",
  });
});
