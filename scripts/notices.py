#!/usr/bin/env python3
"""Helpers for scripts/licenses.sh (`just licenses`): the license files the packages ship.

Subcommands (errors go to stderr with exit status 2):
  rust <about.json> <out.txt>   THIRD-PARTY-NOTICES-rust.txt from `cargo about generate --format json`:
                                the crates grouped by license, each crate with the copyright lines
                                of its license files, each license's text printed once, and where
                                to get the source of the crates under the LGPL (the libmpv bindings)
  copyright <LICENSE> <out>     the .deb's /usr/share/doc/zapper/copyright, in Debian's
                                machine-readable format (DEP-5), with LICENSE's text

zapper has no browser engine of its own to credit (the system's WebKitGTK, WebView2 or WKWebView draws its window).
"""
import json
import os
import re
import sys

# Where zapper's source is: the repository the updater takes its releases from (RELEASES_REPO in
# crates/zapper/src/updates/mod.rs), which scripts/licenses.sh reads through scripts/releases-repo.sh.
REPO = f"https://github.com/{os.environ.get('ZAPPER_REPO', 'francislavoie/zapper')}"


class Fail(Exception):
    pass


# A copyright line: "Copyright ...", "(c) ...", "© ...", optionally behind comment markers. Not the
# license's own mentions ("the above copyright notice", "COPYRIGHT HOLDERS") or a template's
# placeholder ("Copyright [yyyy] [name of copyright owner]", the GPL's "Copyright (C) year  name of
# author"). Nor the license's own copyright (the FSF's, the MPL's "(c) under Patent Claims"): those
# stay in the license text, where they belong.
COPYRIGHT = re.compile(r"^(copyright\b|\(c\)\s|©)", re.I)
NOT_COPYRIGHT = re.compile(
    r"\[yyyy\]|<year>|\{yyyy\}|\[name of copyright owner\]|year\s+name of author|Free Software Foundation, Inc"
    r"|^\(c\)\s+under\b|^copyright\s+(notice|holders?|owner|license|and\b|law)", re.I)


def as_copyright(line):
    """The line as a copyright line (comment markers stripped), or None."""
    s = line.strip().lstrip("#*/;- ").strip()
    return s if COPYRIGHT.match(s) and not NOT_COPYRIGHT.search(s) and len(s) < 300 else None


def copyright_lines(text):
    return [c for c in map(as_copyright, text.splitlines()) if c]


def without_copyright(text):
    """The license text minus its copyright lines (listed per package instead), trimmed."""
    keep = [line.rstrip() for line in text.replace("\r\n", "\n").split("\n") if not as_copyright(line)]
    body = "\n".join(keep).strip("\n")
    return re.sub(r"\n{3,}", "\n\n", body)


def text_key(text):
    """Texts that differ only in copyright lines, a title line, wrapping or punctuation are one."""
    lines = [line for line in without_copyright(text).split("\n") if line.strip()]
    if lines and len(lines[0]) < 60 and re.search(r"licen[cs]e", lines[0], re.I) and not lines[0].rstrip().endswith("."):
        lines = lines[1:]
    return re.sub(r"[^a-z0-9]", "", " ".join(lines).lower())


def render(title, intro, unit, groups, extra=()):
    """groups: {license id: {"packages": {label: [copyright lines] or None}, "authors": {label: str},
    "texts": [(key, text, set(labels))]}} -> the notices file's text, then the `extra` lines."""
    order = sorted(groups, key=lambda i: (-len(groups[i]["packages"]), i))
    width = max(len(i) for i in order) if order else 10
    out = [title, "=" * len(title), "", intro, "", "Summary", "-------"]
    for lid in order:
        out.append(f"  {lid.ljust(width)}  {len(groups[lid]['packages']):>4}")
    out.append("")
    for lid in order:
        g = groups[lid]
        n = len(g["packages"])
        out += ["", RULE, f"{lid} ({n} {unit}{'' if n == 1 else 's'})", RULE, ""]
        for label in sorted(g["packages"], key=str.lower):
            out.append(label)
            lines = g["packages"][label]
            if lines:
                out += [f"  {c}" for c in lines]
            else:
                who = g["authors"].get(label)
                out.append(f"  (no copyright line in its license files{'; authors: ' + who if who else ''})")
        texts = sorted(g["texts"], key=lambda t: (-len(t[2]), t[0]))
        for i, (_, text, users) in enumerate(texts, 1):
            head = f"--- {lid} license text"
            if len(texts) > 1:
                head += f" ({i} of {len(texts)}; used by {', '.join(sorted(users, key=str.lower))})"
            out += ["", head + " ---", "", text, ""]
    out += list(extra)
    return "\n".join(out).rstrip("\n") + "\n"


def add(groups, lid, label, text, authors=None):
    g = groups.setdefault(lid, {"packages": {}, "authors": {}, "texts": []})
    lines = g["packages"].setdefault(label, [])
    for c in copyright_lines(text or ""):
        if c not in lines:
            lines.append(c)
    if authors:
        g["authors"][label] = authors
    if text and text.strip():
        key = text_key(text)
        for k, _, users in g["texts"]:
            if k == key:
                users.add(label)
                break
        else:
            g["texts"].append((key, without_copyright(text), {label}))


LICENSE_FILE = re.compile(r"licen[cs]e|copying|notice|copyright|unlicense|patents", re.I)
COMMENT = re.compile(r"^\s*(//+!?|/\*+|\*+/?|#(?![!\[]))\s?")
# A section's rule, as wide as the license texts are set (80 columns), so Settings > About can
# show it unwrapped.
RULE = "=" * 80
# The LGPL asks that whoever gets the binary can get the library's source, to rebuild against a
# modified copy: those crates get a line saying where it is.
LGPL = re.compile(r"\bLGPL\b")


def comment_block(text):
    """A license header cargo-about found in a source file: its comment lines without the markers
    (the code around it is no notice)."""
    return "\n".join(COMMENT.sub("", line).rstrip() for line in text.split("\n") if COMMENT.match(line))


def rust(about_json, out_path):
    with open(about_json, encoding="utf-8") as f:
        data = json.load(f)
    groups = {}
    sources = {}
    for lic in data["licenses"]:
        text = lic.get("text", "")
        source = os.path.basename(lic.get("source_path") or "")
        if source and not LICENSE_FILE.search(source):
            text = comment_block(text)
        for use in lic["used_by"]:
            c = use["crate"]
            label = f"{c['name']} {c['version']}"
            authors = ", ".join(re.sub(r"\s*<[^>]*>", "", a) for a in c.get("authors") or [])
            add(groups, lic["id"], label, text, authors)
            if LGPL.search(lic["id"]):
                where = [f"  crate: https://crates.io/crates/{c['name']}/{c['version']}"]
                if c.get("repository"):
                    where.append(f"  source repository: {c['repository']}")
                sources[label] = [f"{label} ({lic['id']})", *where]
    if not groups:
        raise Fail(f"no licenses in {about_json}")
    for lid, g in groups.items():
        if not g["texts"]:
            raise Fail(f"no license text for {lid}")
    extra = []
    if sources:
        extra += ["", RULE, "Source code of the crates under the LGPL", RULE, "",
                  "The crates below are compiled into zapper's binary, unmodified. Their license\n"
                  "lets you replace them with a modified version: their source is published at\n"
                  "the addresses given, and zapper's own source, to rebuild it against your copy,\n"
                  f"is at {REPO}."]
        for label in sorted(sources, key=str.lower):
            extra += ["", *sources[label]]
    text = render(
        "zapper: third-party notices for the Rust crates",
        "zapper's binary is built from the Rust crates below. Each crate is listed under\n"
        "the license zapper uses it under, with the copyright lines from its license\n"
        "files; the full text of each license follows its list. Generated by\n"
        "`just licenses` (cargo-about) from Cargo.lock.",
        "crate", groups, extra)
    with open(out_path, "w", encoding="utf-8") as f:
        f.write(text)


def copyright(license_path, out_path):
    with open(license_path, encoding="utf-8") as f:
        license_text = f.read()
    m = re.search(r"^Copyright \(c\) (.+)$", license_text, re.M)
    if not license_text.startswith("MIT License") or not m:
        raise Fail("LICENSE isn't the MIT license with a 'Copyright (c) <year> <holder>' line")
    body = license_text.split(m.group(0), 1)[1].strip("\n")
    text = "\n".join(f" {line}" if line.strip() else " ." for line in body.split("\n"))
    with open(out_path, "w", encoding="utf-8") as f:
        f.write(f"""Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: zapper
Source: {REPO}
Comment: The binary also contains Rust crates and npm packages under their own licenses; each
 one, with its copyright lines and license text, is listed in THIRD-PARTY-NOTICES-rust.txt and
 THIRD-PARTY-NOTICES-ui.txt in this directory. Video is played by libmpv, which this package
 depends on and does not contain: MPV-FFMPEG-NOTICE.txt says where its license and source are.

Files: *
Copyright: {m.group(1)}
License: MIT

License: MIT
{text}
""")


def main(argv):
    cmd, args = (argv[1], argv[2:]) if len(argv) > 1 else ("", [])
    if cmd == "rust" and len(args) == 2:
        rust(*args)
    elif cmd == "copyright" and len(args) == 2:
        copyright(*args)
    else:
        raise Fail(__doc__.split("Subcommands")[1].split("\n\n")[0])


if __name__ == "__main__":
    try:
        main(sys.argv)
    except Fail as e:
        print(f"notices: {e}", file=sys.stderr)
        sys.exit(2)
