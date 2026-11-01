// Account kinds: the capability comparison the settings pane shows, and the
// playlist form's keep-or-replace rules — which are the playlist's version of
// the write-only password, so getting them wrong either loses a saved
// address or sends `<redacted>` back as if it were one.
//
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import {
  SOURCE_COMPARISON,
  SOURCE_KINDS,
  asKind,
  kindInfo,
  m3uChanged,
  m3uFormFrom,
  planM3uSave,
  splitUrls,
} from "../logic/sources.ts";

const onFile = (p = {}) => ({
  playlist: "",
  playlistRedacted: false,
  epgUrls: [],
  epgRedacted: false,
  userAgent: "",
  ...p,
});

test("there are exactly the two kinds the backend knows, each labelled once", () => {
  assert.deepEqual(
    SOURCE_KINDS.map((k) => k.id),
    ["xtream", "m3u"],
  );
  const labels = SOURCE_KINDS.map((k) => k.label);
  assert.equal(new Set(labels).size, labels.length);
  for (const k of SOURCE_KINDS) assert.ok(k.blurb.length > 0, k.id);
});

test("every comparison row answers for both kinds, briefly", () => {
  assert.ok(SOURCE_COMPARISON.length >= 4);
  const aspects = SOURCE_COMPARISON.map((r) => r.aspect);
  assert.equal(new Set(aspects).size, aspects.length, "duplicate row");
  for (const r of SOURCE_COMPARISON) {
    for (const cell of [r.aspect, r.xtream, r.m3u]) {
      assert.ok(cell.trim().length > 0, `empty cell in ${r.aspect}`);
      // Scannable means short: a cell that wraps to four lines in a 40rem
      // table is a paragraph, and nobody compares paragraphs.
      assert.ok(cell.length <= 70, `too long to scan: ${cell}`);
    }
    assert.notEqual(r.xtream, r.m3u, `${r.aspect} says the same thing twice`);
  }
});

test("the comparison is honest about what a playlist does not get", () => {
  // These are the differences the backend actually has; if one of them stops
  // being true, this row is what has to change.
  const row = (a) => SOURCE_COMPARISON.find((r) => r.aspect === a);
  assert.match(row("Guide").m3u, /XMLTV/);
  assert.match(row("Catch-up").m3u, /catchup=/);
  assert.match(row("Categories").m3u, /group-title/);
  assert.match(row("Account info").m3u, /no status/i);
});

test("an unknown or missing kind reads as a panel", () => {
  assert.equal(asKind("m3u"), "m3u");
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

test("the user agent is trimmed, and a changed one is a change", () => {
  const a = onFile({ playlist: "/tv.m3u", userAgent: "Kodi/21" });
  const form = m3uFormFrom(a);
  assert.equal(m3uChanged({ ...form, userAgent: " Kodi/21 " }, a), false);
  assert.equal(m3uChanged({ ...form, userAgent: "" }, a), true);
  assert.equal(planM3uSave({ ...form, userAgent: " TiviMate " }, a).userAgent, "TiviMate");
});
