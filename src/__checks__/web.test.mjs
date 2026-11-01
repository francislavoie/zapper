import test from "node:test";
import assert from "node:assert/strict";
import {
  BROWSER_SETTINGS,
  CSRF_HEADER,
  apiUrl,
  browserSettings,
  errorText,
  idList,
  requestInit,
  signedOutStatus,
  syncPollMs,
  withBrowserSettings,
} from "../logic/web.ts";
import { DEFAULT_SETTINGS } from "../logic/settings.ts";

test("addresses leave out what is not given and encode what is", () => {
  assert.equal(apiUrl("/api/categories"), "/api/categories");
  assert.equal(
    apiUrl("/api/channels", { category: null, offset: 0, limit: 200 }),
    "/api/channels?offset=0&limit=200",
  );
  assert.equal(
    apiUrl("/api/channels", { category: "news & sport", offset: 50, limit: 50 }),
    "/api/channels?category=news%20%26%20sport&offset=50&limit=50",
  );
  // `a|b` reaches the service as typed, to match either.
  assert.equal(
    apiUrl("/api/channels/search", { q: "one|two", limit: 5 }),
    "/api/channels/search?q=one%7Ctwo&limit=5",
  );
  assert.equal(apiUrl("/x", { a: undefined }), "/x");
  assert.equal(idList([3, 1, 2]), "3,1,2");
});

test("a change carries the header; a read does not need it", () => {
  const read = requestInit("GET");
  assert.equal(read.method, "GET");
  assert.equal(read.credentials, "same-origin");
  assert.equal(read.headers[CSRF_HEADER], undefined);
  assert.equal(read.body, undefined);

  for (const method of ["POST", "PUT", "DELETE"]) {
    const init = requestInit(method);
    assert.equal(init.headers[CSRF_HEADER], "1", method);
    assert.equal(init.credentials, "same-origin", method);
  }
  const body = requestInit("POST", { password: "x", cookie: true });
  assert.equal(body.headers["Content-Type"], "application/json");
  assert.deepEqual(JSON.parse(body.body), { password: "x", cookie: true });
  // Never a token: the cookie is the browser's to send, and nothing here
  // reads or writes one.
  assert.equal(body.headers.Authorization, undefined);
});

test("a failure says what the service said, or what happened", () => {
  assert.equal(errorText(401, { error: "wrong password" }), "wrong password");
  assert.equal(errorText(500, { error: "  " }), "The recording service answered 500.");
  assert.equal(errorText(502, null), "The recording service answered 502.");
  assert.equal(errorText(0, null), "The recording service could not be reached.");
  assert.ok(signedOutStatus(401));
  assert.ok(!signedOutStatus(403), "403 is a refusal, not a lost sign-in");
});

test("the sync is asked about often while it works, rarely otherwise", () => {
  const at = (phase) => ({ phase, message: "", channelsTotal: 0, lastSynced: null });
  for (const p of ["categories", "channels", "epg"]) assert.equal(syncPollMs(at(p)), 2000, p);
  for (const p of ["idle", "failed"]) assert.equal(syncPollMs(at(p)), 15000, p);
  assert.equal(syncPollMs(null), 15000);
});

test("a browser keeps how its guide looks, and nothing else", () => {
  assert.deepEqual([...BROWSER_SETTINGS], ["guideHours", "timeFormat"]);
  const mine = withBrowserSettings(DEFAULT_SETTINGS, { guideHours: 5, timeFormat: "24h" });
  assert.equal(mine.guideHours, 5);
  assert.equal(mine.timeFormat, "24h");
  assert.equal(mine.rewindMb, DEFAULT_SETTINGS.rewindMb);
  // Whatever else storage holds is not taken.
  const junk = withBrowserSettings(DEFAULT_SETTINGS, {
    guideHours: "lots",
    timeFormat: "36h",
    simultaneousStreams: 9,
    syncIntervalHours: 1,
  });
  assert.deepEqual(junk, DEFAULT_SETTINGS);
  assert.deepEqual(withBrowserSettings(DEFAULT_SETTINGS, null), DEFAULT_SETTINGS);
  assert.deepEqual(withBrowserSettings(DEFAULT_SETTINGS, "x"), DEFAULT_SETTINGS);
  assert.deepEqual(browserSettings({ ...DEFAULT_SETTINGS, guideHours: 4, simultaneousStreams: 3 }), {
    guideHours: 4,
    timeFormat: DEFAULT_SETTINGS.timeFormat,
  });
});
