// The license notices (docs/licensing.md): the allow-list the build holds every bundled package
// to, the notices it writes, the order Settings > About lists them in, and the packages that ship
// them.
//
// The allow-list is the part that must not drift: a GPL package slipping into the interface, or
// a notice the build writes that no package carries, is exactly the mistake nobody sees.
// Run: node --test src/__checks__/

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  acceptedFromAboutToml,
  buildUiNotices,
  chooseLicense,
  noticeFiles,
  packageDirOf,
} from "../../tools/licenses.mjs";
import { isPageFallback, NOTICE_LABELS, noticeLabel, orderNotices } from "../logic/licenses.ts";

const root = fileURLToPath(new URL("../../", import.meta.url));
const read = (p) => readFileSync(join(root, p), "utf8");
const accepted = acceptedFromAboutToml(read("about.toml"));

test("the allow-list is about.toml's, permissive, and has no copyleft license in it", () => {
  for (const id of ["MIT", "Apache-2.0", "BSD-3-Clause", "ISC", "MPL-2.0"]) assert.ok(accepted.includes(id), id);
  for (const id of accepted) assert.doesNotMatch(id, /GPL/, `${id} must not be accepted in general`);
  // The libmpv bindings' LGPL is accepted for those two crates alone, in their own sections.
  const toml = read("about.toml");
  assert.match(toml, /^\[libmpv2\]\naccepted = \["LGPL-2\.1"\]$/m);
  assert.match(toml, /^\[libmpv2-sys\]\naccepted = \["LGPL-2\.1"\]$/m);
});

test("chooseLicense takes the earliest accepted alternative, and needs every part of an AND", () => {
  assert.deepEqual(chooseLicense("Apache-2.0 OR MIT", accepted), ["MIT"]);
  assert.deepEqual(chooseLicense("(MIT OR GPL-3.0) AND ISC", accepted), ["MIT", "ISC"]);
  assert.equal(chooseLicense("GPL-3.0-only", accepted), null);
  assert.equal(chooseLicense("MIT AND GPL-2.0", accepted), null);
  assert.equal(chooseLicense("", accepted), null);
});

test("packageDirOf finds the package a bundled module is from, scoped or not", () => {
  assert.deepEqual(packageDirOf("/r/node_modules/react/index.js"), { dir: "/r/node_modules/react", name: "react" });
  assert.deepEqual(packageDirOf("/r/node_modules/@tauri-apps/api/core.js?v=1"), {
    dir: "/r/node_modules/@tauri-apps/api",
    name: "@tauri-apps/api",
  });
  assert.equal(packageDirOf("/r/src/App.tsx"), null);
});

test("the UI notices list each bundled package with its copyright, and refuse one outside the list", () => {
  const ids = [
    join(root, "node_modules/react/index.js"),
    join(root, "node_modules/react/cjs/react.production.js"),
    join(root, "src/App.tsx"),
  ];
  const ok = buildUiNotices({ moduleIds: ids, accepted, exceptions: {}, root });
  assert.deepEqual(ok.errors, []);
  assert.deepEqual(ok.counts, { MIT: 1 });
  assert.match(ok.text, /^react \d+\.\d+\.\d+\n  Copyright \(c\) Meta Platforms/m);
  assert.match(ok.text, /--- MIT license text ---/);

  // The same package, with an allow-list that doesn't have its license: the build's failure.
  const refused = buildUiNotices({ moduleIds: ids, accepted: ["ISC"], exceptions: {}, root });
  assert.deepEqual(refused.errors.map((e) => e.replace(/ \d.*?:/, ":")), ["react: MIT"]);
});

test("every notice the build can write has a label in the About list, and every label a file", () => {
  const written = new Set([
    "THIRD-PARTY-NOTICES-ui.txt",
    ...noticeFiles({ platform: "linux" }).map((f) => f.name),
    ...noticeFiles({ platform: "win32" }).map((f) => f.name),
  ]);
  assert.deepEqual([...written].sort(), Object.keys(NOTICE_LABELS).sort());
});

test("the GPL's text is a Windows build's alone: the platform whose installers carry libmpv-2.dll", () => {
  assert.ok(noticeFiles({ platform: "win32" }).some((f) => f.name === "GPL-3.0.txt"));
  for (const platform of ["linux", "darwin"]) {
    assert.ok(!noticeFiles({ platform }).some((f) => f.name === "GPL-3.0.txt"), platform);
  }
  assert.equal(
    noticeFiles({ licensesDir: "/x", platform: "linux" }).find((f) => f.name === "THIRD-PARTY-NOTICES-rust.txt").path,
    "/x/THIRD-PARTY-NOTICES-rust.txt",
  );
});

test("the About list puts zapper first and an unknown file last, under its own name", () => {
  const files = ["zz-extra.txt", "THIRD-PARTY-NOTICES-ui.txt", "THIRD-PARTY-NOTICES-rust.txt", "MPV-FFMPEG-NOTICE.txt", "LICENSE.txt"]
    .map((name) => ({ name, size: 1 }));
  assert.deepEqual(orderNotices(files).map((f) => f.name), [
    "LICENSE.txt",
    "MPV-FFMPEG-NOTICE.txt",
    "THIRD-PARTY-NOTICES-rust.txt",
    "THIRD-PARTY-NOTICES-ui.txt",
    "zz-extra.txt",
  ]);
  assert.deepEqual(noticeLabel("zz-extra.txt"), { label: "zz-extra.txt", detail: null });
  assert.equal(noticeLabel("LICENSE.txt").label, "zapper");
});

test("a missing notice answered with the app's page is recognised, not shown as text", () => {
  assert.equal(isPageFallback("text/html; charset=utf-8", ""), true);
  assert.equal(isPageFallback(null, "<!DOCTYPE html>\n<html>"), true);
  assert.equal(isPageFallback("text/plain", "MIT License"), false);
});

// Each package carries every notice that applies to it, from where the build leaves it.
test("the .deb, the .rpm, the Windows installers and the macOS app all carry the notices", () => {
  const conf = JSON.parse(read("src-tauri/tauri.conf.json"));
  const everywhere = ["LICENSE", "MPV-FFMPEG-NOTICE.txt", "THIRD-PARTY-NOTICES-rust.txt", "THIRD-PARTY-NOTICES-ui.txt"];
  const deb = conf.bundle.linux.deb.files;
  for (const f of [...everywhere, "copyright"]) assert.ok(deb[`/usr/share/doc/zapper/${f}`], `.deb: ${f}`);
  const rpm = conf.bundle.linux.rpm.files;
  for (const f of everywhere) assert.ok(rpm[`/usr/share/licenses/zapper/${f}`], `.rpm: ${f}`);

  const resources = (p) => Object.values(JSON.parse(read(p)).bundle.resources);
  const win = resources("packaging/windows/installers.conf.json");
  for (const f of ["LICENSE.txt", "MPV-FFMPEG-NOTICE.txt", "GPL-3.0.txt", "THIRD-PARTY-NOTICES-rust.txt", "THIRD-PARTY-NOTICES-ui.txt"]) {
    assert.ok(win.includes(`licenses/${f}`), `Windows: ${f}`);
  }
  const mac = resources("packaging/macos/dmg.conf.json");
  for (const f of ["LICENSE.txt", "MPV-FFMPEG-NOTICE.txt", "THIRD-PARTY-NOTICES-rust.txt", "THIRD-PARTY-NOTICES-ui.txt"]) {
    assert.ok(mac.includes(`licenses/${f}`), `macOS: ${f}`);
  }
  // The DLL is what the GPL's text is for; it ships beside the notices.
  assert.ok(Object.values(JSON.parse(read("src-tauri/tauri.windows.conf.json")).bundle.resources).includes("libmpv-2.dll"));
});

test("zapper is MIT everywhere it says so", () => {
  assert.match(read("LICENSE"), /^MIT License\n\nCopyright \(c\) 2026 Francis Lavoie\n/);
  assert.equal(JSON.parse(read("package.json")).license, "MIT");
  assert.match(read("src-tauri/Cargo.toml"), /^license = "MIT"$/m);
  assert.equal(JSON.parse(read("src-tauri/tauri.conf.json")).bundle.license, "MIT");
});
