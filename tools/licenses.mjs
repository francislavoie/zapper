// The UI's third-party notices (docs/licensing.md): every npm package with code in the production
// bundle, found from the bundle's own module list, so devDependencies (Vite, TypeScript, the Tauri
// CLI) and tree-shaken packages never appear and nothing bundled is missed. Each package is held
// to the allow-list in ../about.toml, the one the Rust notices use. The build writes
// dist/licenses/THIRD-PARTY-NOTICES-ui.txt, copies in the other notices (noticeFiles: zapper's
// LICENSE, libmpv's, the Rust crates'), lists them all in dist/licenses/index.json for Settings >
// About, and fails on a license outside the allow-list that tools/license-exceptions.json doesn't
// name.
//
// Plain JavaScript rather than TypeScript: vite.config.ts imports it, and the project's tsc has no
// Node types to check a file that reads the disk (licenses.d.mts types what the config uses), and
// the node checks import it as it is (src/__checks__/licenses.test.mjs).
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { isAbsolute, join } from "node:path";

/** The `accepted = [...]` list of cargo-about's about.toml, in order. */
export function acceptedFromAboutToml(toml) {
  const m = /^accepted\s*=\s*\[([\s\S]*?)\]/m.exec(toml);
  if (!m) throw new Error("about.toml has no accepted = [...] list");
  return [...m[1].replace(/#.*$/gm, "").matchAll(/"([^"]+)"/g)].map((x) => x[1]);
}

/** The licenses an SPDX expression is used under: for OR the alternative earliest in `accepted`,
 *  for AND every part. Null when no choice is fully accepted. */
export function chooseLicense(expr, accepted) {
  const tokens = expr.match(/\(|\)|[^\s()]+/g) ?? [];
  let i = 0;
  const rank = (ids) => Math.min(...ids.map((id) => accepted.indexOf(id)));
  // Each alternative is a list of licenses that must all be accepted.
  function parseOr() {
    const alts = parseAnd();
    while (tokens[i] === "OR") {
      i++;
      alts.push(...parseAnd());
    }
    return alts;
  }
  function parseAnd() {
    let alts = parseAtom();
    while (tokens[i] === "AND") {
      i++;
      const right = parseAtom();
      alts = alts.flatMap((a) => right.map((b) => [...a, ...b]));
    }
    return alts;
  }
  function parseAtom() {
    const t = tokens[i++];
    if (t === "(") {
      const r = parseOr();
      i++;
      return r;
    }
    if (t === undefined) return [["(no license)"]];
    let id = t;
    if (tokens[i] === "WITH") {
      id = `${t} WITH ${tokens[i + 1]}`;
      i += 2;
    }
    return [[id]];
  }
  if (!tokens.length) return null;
  const alts = parseOr().filter((a) => a.every((id) => accepted.includes(id)));
  if (!alts.length || i < tokens.length) return null;
  alts.sort((a, b) => rank(a) - rank(b));
  return [...new Set(alts[0])];
}

/** A module id as a `/`-separated path (an id on Windows may have `\`), without the virtual-module
 *  `\0` or a query. */
function modulePath(id) {
  return id.replace(/^\0/, "").replace(/[?#].*$/, "").replace(/\\/g, "/");
}

/** The node_modules package a bundled module belongs to, or null (our own code, virtual modules). */
export function packageDirOf(id) {
  const path = modulePath(id);
  const at = path.lastIndexOf("/node_modules/");
  if (at < 0) return null;
  const rest = path.slice(at + "/node_modules/".length).split("/");
  const name = rest[0].startsWith("@") ? `${rest[0]}/${rest[1]}` : rest[0];
  return { dir: path.slice(0, at + "/node_modules/".length) + name, name };
}

const COPYRIGHT = /^(copyright\b|\(c\)\s|©)/i;
const NOT_COPYRIGHT =
  /\[yyyy\]|<year>|\{yyyy\}|\[name of copyright owner\]|year\s+name of author|Free Software Foundation, Inc|^\(c\)\s+under\b|^copyright\s+(notice|holders?|owner|license|and\b|law|statements?)/i;
const asCopyright = (line) => {
  const s = line.trim().replace(/^[#*/;\- ]+/, "").trim();
  return COPYRIGHT.test(s) && !NOT_COPYRIGHT.test(s) && s.length < 300 ? s : null;
};

export function copyrightLines(text) {
  return text.split(/\r?\n/).map(asCopyright).filter((c) => !!c);
}

function withoutCopyright(text) {
  return text
    .replace(/\r\n/g, "\n")
    .split("\n")
    .filter((l) => !asCopyright(l))
    .map((l) => l.trimEnd())
    .join("\n")
    .replace(/^\n+|\n+$/g, "")
    .replace(/\n{3,}/g, "\n\n");
}

/** Texts that differ only in copyright lines, a title line, wrapping or punctuation are one. */
function textKey(text) {
  let lines = withoutCopyright(text).split("\n").filter((l) => l.trim());
  if (lines.length && lines[0].length < 60 && /licen[cs]e/i.test(lines[0]) && !lines[0].trimEnd().endsWith(".")) {
    lines = lines.slice(1);
  }
  return lines.join(" ").toLowerCase().replace(/[^a-z0-9]/g, "");
}

const LICENSE_FILE = /^(licen[cs]e|copying|unlicense)([-._ ].*)?$/i;
const NOTICE_FILE = /^(notice|third[-_ ]?party[-_ ]?notices)([-._ ].*)?$/i;
/** A section's rule, as wide as the license texts are set (80 columns), so Settings > About can
 *  show it unwrapped. */
const RULE = "=".repeat(80);
const NOT_TEXT = /\.(spdx|json|js|mjs|cjs|ts|html?)$/i;

function readText(dir, f) {
  const p = join(dir, f);
  try {
    return statSync(p).isFile() ? readFileSync(p, "utf8") : null;
  } catch {
    return null;
  }
}

/** A package's license texts for the licenses it's used under: with several files, those named
 *  for a chosen license, else all but those named for another one (LICENSE-MPL when Apache-2.0
 *  is used). */
function licenseTexts(dir, chosen) {
  const files = readdirSync(dir).filter((f) => LICENSE_FILE.test(f) && !NOT_TEXT.test(f)).sort();
  const token = (id) => id.toLowerCase().split(/[-.\s]/)[0];
  const wanted = chosen.map(token);
  const named = (f) =>
    ["mit", "apache", "bsd", "isc", "mpl", "zlib", "cc0", "unicode", "ofl", "bsl"].filter((t) => f.toLowerCase().includes(t));
  let pick = files;
  if (files.length > 1) {
    const match = files.filter((f) => named(f).some((t) => wanted.includes(t)));
    pick = match.length ? match : files.filter((f) => !named(f).length);
    if (!pick.length) pick = files;
  }
  return pick.map((f) => readText(dir, f)).filter((t) => !!t?.trim());
}

/** Licenses whose terms ask that recipients can get the covered files' source: a package under one
 *  gets its npm page and repository listed. None is bundled today; the list is here so one that
 *  arrives is pointed to rather than only named. */
const WEAK_COPYLEFT_SOURCE = new Set(["MPL-2.0", "EPL-2.0"]);

function sourceNote(label, name, pkg, licenses) {
  const repo = pkg.repository;
  const raw = typeof repo === "string" ? repo : repo && typeof repo === "object" && "url" in repo ? String(repo.url) : "";
  const url = raw.replace(/^git\+/, "").replace(/^git:\/\//, "https://").replace(/\.git$/, "");
  return [
    `${label} (${licenses.join(", ")})`,
    `  npm package: https://www.npmjs.com/package/${name}/v/${String(pkg.version ?? "")}`,
    ...(url ? [`  source repository: ${url}`] : []),
  ];
}

function render(groups, extra) {
  const order = [...groups.keys()].sort(
    (a, b) => groups.get(b).packages.size - groups.get(a).packages.size || a.localeCompare(b),
  );
  const width = Math.max(10, ...order.map((l) => l.length));
  const title = "zapper: third-party notices for the interface";
  const out = [
    title,
    "=".repeat(title.length),
    "",
    "zapper's interface bundles the npm packages below. Each package is listed under\n" +
      "the license zapper uses it under, with the copyright lines from its license\n" +
      "files; the full text of each license follows its list, and notices the packages\n" +
      "ship come after. Generated by the interface build (tools/licenses.mjs) from the\n" +
      "modules in the bundle.",
    "",
    "Summary",
    "-------",
    ...order.map((l) => `  ${l.padEnd(width)}  ${String(groups.get(l).packages.size).padStart(4)}`),
    "",
  ];
  for (const lid of order) {
    const g = groups.get(lid);
    const n = g.packages.size;
    out.push("", RULE, `${lid} (${n} package${n === 1 ? "" : "s"})`, RULE, "");
    for (const label of [...g.packages.keys()].sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()))) {
      out.push(label);
      const lines = g.packages.get(label);
      if (Array.isArray(lines) && lines.length) out.push(...lines.map((c) => `  ${c}`));
      else {
        const none = Array.isArray(lines) ? "" : lines.none;
        out.push(
          `  (${Array.isArray(lines) ? "no copyright line in its license file" : "no license file in the package"}${none ? `; author: ${none}` : ""})`,
        );
      }
    }
    const texts = [...g.texts].sort((a, b) => b.users.size - a.users.size || a.key.localeCompare(b.key));
    texts.forEach((t, i) => {
      let head = `--- ${lid} license text`;
      if (texts.length > 1) head += ` (${i + 1} of ${texts.length}; used by ${[...t.users].sort().join(", ")})`;
      out.push("", `${head} ---`, "", t.text, "");
    });
  }
  out.push(...extra);
  return out.join("\n").replace(/\n+$/, "") + "\n";
}

function authorName(a) {
  const s = typeof a === "string" ? a : a && typeof a === "object" && "name" in a ? String(a.name) : "";
  return s.replace(/\s*[<(][^>)]*[>)]/g, "").trim();
}

function declaredLicense(pkg) {
  const l = pkg.license ?? pkg.licenses;
  if (typeof l === "string") return l;
  if (Array.isArray(l)) return l.map((x) => (typeof x === "string" ? x : x?.type)).filter(Boolean).join(" OR ");
  if (l && typeof l === "object" && "type" in l) return String(l.type);
  return "";
}

/** The licenses a package keeps beside files bundled from one of its subfolders. */
function subfolderLicenses(pkgDir, label, subFiles) {
  const out = [];
  for (const [dir, files] of [...subFiles].filter(([d]) => d.startsWith(`${pkgDir}/`)).sort()) {
    let names;
    try {
      names = readdirSync(dir).filter((f) => LICENSE_FILE.test(f) && !NOT_TEXT.test(f)).sort();
    } catch {
      continue;
    }
    const rel = dir.slice(pkgDir.length + 1);
    for (const f of names) {
      const parts = f.replace(/\.(txt|md)$/i, "").split(/[_.-]/).slice(1).map((x) => x.toLowerCase());
      const users = [...files].filter((b) => !parts.length || parts.some((x) => b.toLowerCase().startsWith(x))).sort();
      const t = users.length ? readText(dir, f) : null;
      if (t?.trim()) {
        out.push("", RULE, `License of ${users.map((u) => `${rel}/${u}`).join(", ")} in ${label} (${rel}/${f})`, RULE, "", t.trim(), "");
      }
    }
  }
  return out;
}

/**
 * The notices for the bundled modules.
 * @param {{ moduleIds: Iterable<string>, accepted: string[], exceptions: { packages?: Record<string, { license: string, reason: string }> }, root?: string }} opts
 * @returns {{ text: string, errors: string[], counts: Record<string, number> }}
 */
export function buildUiNotices(opts) {
  const { accepted, exceptions } = opts;
  const errors = [];
  const pkgs = new Map(); // dir -> name
  const subFiles = new Map(); // a package's subfolder -> the files bundled from it
  for (const id of opts.moduleIds) {
    const path = modulePath(id);
    // The bundler's own helpers are virtual modules: \0vite/preload-helper.js, \0rolldown/runtime.js.
    const helper = /^\0(vite|rolldown)\//.exec(id);
    const p = helper && opts.root ? { dir: join(opts.root, "node_modules", helper[1]).replace(/\\/g, "/"), name: helper[1] } : packageDirOf(id);
    if (!p) continue;
    pkgs.set(p.dir, p.name);
    const slash = path.lastIndexOf("/");
    if (!helper && path.slice(0, slash) !== p.dir) {
      subFiles.set(path.slice(0, slash), (subFiles.get(path.slice(0, slash)) ?? new Set()).add(path.slice(slash + 1)));
    }
  }

  const groups = new Map();
  const counts = {};
  const notices = [];
  const sources = [];
  for (const [dir, name] of [...pkgs].sort((a, b) => a[1].localeCompare(b[1]) || a[0].localeCompare(b[0]))) {
    let pkg;
    try {
      pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
    } catch {
      errors.push(`${name}: no package.json in ${dir}`);
      continue;
    }
    const label = `${name} ${pkg.version ?? "?"}`;
    const declared = declaredLicense(pkg) || exceptions.packages?.[name]?.license || "";
    const chosen = chooseLicense(declared, accepted);
    if (!chosen) {
      errors.push(`${label}: ${declared || "(no license)"}`);
      continue;
    }
    const texts = licenseTexts(dir, chosen);
    const weak = chosen.filter((l) => WEAK_COPYLEFT_SOURCE.has(l));
    if (weak.length) sources.push(sourceNote(label, name, pkg, weak));
    for (const lid of chosen) {
      counts[lid] = (counts[lid] ?? 0) + 1;
      const g = groups.get(lid) ?? { packages: new Map(), texts: [] };
      groups.set(lid, g);
      const lines = [...new Set(texts.flatMap(copyrightLines))];
      g.packages.set(label, texts.length ? lines : { none: authorName(pkg.author) });
      for (const text of texts) {
        const key = textKey(text);
        const same = g.texts.find((t) => t.key === key);
        if (same) same.users.add(label);
        else g.texts.push({ key, text: withoutCopyright(text), users: new Set([label]) });
      }
    }
    for (const f of readdirSync(dir).filter((f) => NOTICE_FILE.test(f) && !NOT_TEXT.test(f)).sort()) {
      const t = readText(dir, f);
      if (t?.trim()) notices.push("", RULE, `Notices shipped by ${label} (${f})`, RULE, "", t.trim(), "");
    }
    notices.push(...subfolderLicenses(dir, label, subFiles));
  }

  const extra = [];
  if (sources.length) {
    extra.push(
      "",
      RULE,
      "Source code of the packages under a file-level copyleft license",
      RULE,
      "",
      "The packages below ship in zapper as minified JavaScript, from the npm package\n" +
        "named. Their license requires telling you how to get their source: each is\n" +
        "published, unmodified, at the address given.",
    );
    for (const s of sources) extra.push("", ...s);
  }
  extra.push(...notices);
  return { text: render(groups, extra), errors, counts };
}

/**
 * The files Settings > About > Open source licenses lists beside the UI's own notices, as the
 * build copies them into dist/licenses/ (a `.txt` name: the app's asset protocol answers a path
 * with no extension with index.html). Paths are from the repository root. A file that doesn't
 * exist (the Rust notices before `just licenses` has run) is left out, and the pane lists what's
 * there. GPL-3.0.txt only in a Windows build: that is the platform whose installers carry
 * libmpv-2.dll, which is under it (packaging/licenses/MPV-FFMPEG-NOTICE.txt).
 * @param {{ licensesDir?: string, platform?: string }} [env]
 */
export function noticeFiles({
  licensesDir = process.env.ZAPPER_LICENSES_DIR ?? "src-tauri/target/licenses",
  platform = process.env.ZAPPER_LICENSES_PLATFORM ?? process.platform,
} = {}) {
  return [
    { name: "LICENSE.txt", path: "LICENSE" },
    { name: "MPV-FFMPEG-NOTICE.txt", path: "packaging/licenses/MPV-FFMPEG-NOTICE.txt" },
    ...(platform === "win32" ? [{ name: "GPL-3.0.txt", path: "packaging/licenses/GPL-3.0.txt" }] : []),
    { name: "THIRD-PARTY-NOTICES-rust.txt", path: `${licensesDir}/THIRD-PARTY-NOTICES-rust.txt` },
  ];
}

/**
 * The Vite plugin: at the end of the production build, the UI's notices and noticeFiles() into
 * dist/licenses/, with index.json naming them. The allow-list is about.toml's, the exceptions
 * tools/license-exceptions.json, both from the project root Vite builds in.
 * @returns {import("vite").Plugin}
 */
export function licenseNotices() {
  let root = process.cwd();
  return {
    name: "zapper-license-notices",
    apply: "build",
    configResolved(config) {
      root = config.root;
    },
    generateBundle(_options, bundle) {
      const ids = new Set();
      for (const out of Object.values(bundle)) if (out.type === "chunk") for (const id of out.moduleIds ?? []) ids.add(id);
      const accepted = acceptedFromAboutToml(readFileSync(join(root, "about.toml"), "utf8"));
      const exceptions = JSON.parse(readFileSync(join(root, "tools", "license-exceptions.json"), "utf8"));
      const r = buildUiNotices({ moduleIds: ids, accepted, exceptions, root });
      if (r.errors.length) {
        this.error(
          `licenses: bundled packages under a license outside the allow-list in about.toml, or unclear (see docs/licensing.md):\n  ${r.errors.join("\n  ")}`,
        );
      }
      const files = [];
      const emit = (name, source) => {
        this.emitFile({ type: "asset", fileName: `licenses/${name}`, source });
        files.push({ name, size: typeof source === "string" ? Buffer.byteLength(source) : source.length });
      };
      emit("THIRD-PARTY-NOTICES-ui.txt", r.text);
      for (const f of noticeFiles()) {
        const p = isAbsolute(f.path) ? f.path : join(root, f.path);
        if (existsSync(p)) emit(f.name, readFileSync(p));
      }
      this.emitFile({ type: "asset", fileName: "licenses/index.json", source: JSON.stringify({ files }) });
      this.info?.(
        `licenses: ${Object.entries(r.counts)
          .sort((a, b) => b[1] - a[1])
          .map(([l, n]) => `${l} ${n}`)
          .join(", ")}`,
      );
    },
  };
}
