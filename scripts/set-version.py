#!/usr/bin/env python3
"""Usage: scripts/set-version.py <version>  (part of `just release`)

Sets zapper's version everywhere it's recorded, editing only the version lines (no
reformatting): [workspace.package] in Cargo.toml (zapper's crate inherits it) and zapper's own entry in Cargo.lock (what the updater compares
against: CARGO_PKG_VERSION), package.json, and package-lock.json's two copies. Each file is
parsed again afterwards and must differ from the original only in those versions. Errors exit
with status 2 before any file is written.
"""
import json
import os
import re
import sys
import tomllib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRATE = "zapper"


class Fail(Exception):
    pass


def sub_once(text, pattern, version, what):
    """Replaces group 2 of the single first match of pattern (groups: prefix, old, suffix)."""
    m = re.search(pattern, text, re.M)
    if not m:
        raise Fail(f"can't find {what}")
    return text[:m.start(2)] + version + text[m.end(2):]


def json_edit(rel, version, edits):
    """edits: (pattern, key path) pairs, each pattern matching one version line."""
    path = os.path.join(ROOT, rel)
    text = open(path, encoding="utf-8").read()
    want = json.loads(text)
    for pattern, keys in edits:
        text = sub_once(text, pattern, version, f"{'.'.join(keys) or '?'} in {rel}")
        node = want
        for k in keys[:-1]:
            node = node[k]
        if keys[-1] not in node:
            raise Fail(f"{rel} has no {'.'.join(keys)}")
        node[keys[-1]] = version
    if json.loads(text) != want:
        raise Fail(f"editing {rel} changed more than its version")
    return path, text


def main(argv):
    if len(argv) != 2:
        raise Fail(__doc__.splitlines()[0])
    version = argv[1]
    if not re.fullmatch(r"[0-9A-Za-z.+-]+", version):
        raise Fail(f"unexpected characters in {version!r}")
    writes = []

    writes.append(json_edit("package.json", version, [(r'^(  "version": ")([^"]*)(")', ["version"])]))
    writes.append(json_edit("package-lock.json", version, [
        (r'^(  "version": ")([^"]*)(")', ["version"]),
        (r'^(    "": \{\n(?:      .*\n)*?      "version": ")([^"]*)(")', ["packages", "", "version"]),
    ]))

    cargo_toml = os.path.join(ROOT, "Cargo.toml")
    text = open(cargo_toml, encoding="utf-8").read()
    want = tomllib.loads(text)
    want["workspace"]["package"]["version"] = version
    text = sub_once(text, r'^(\[workspace\.package\]\n(?:[^\[\n].*\n|\n)*?version = ")([^"]*)(")', version,
                    "[workspace.package] version in Cargo.toml")
    if tomllib.loads(text) != want:
        raise Fail("editing Cargo.toml changed more than its version")
    writes.append((cargo_toml, text))

    lock_path = os.path.join(ROOT, "Cargo.lock")
    text = open(lock_path, encoding="utf-8").read()
    want = tomllib.loads(text)
    for p in want["package"]:
        if p["name"] == CRATE and "source" not in p:
            p["version"] = version
    text = sub_once(text, rf'^(\[\[package\]\]\nname = "{CRATE}"\nversion = ")([^"]*)(")', version,
                    f"{CRATE} in Cargo.lock")
    if tomllib.loads(text) != want:
        raise Fail("editing Cargo.lock changed more than zapper's version")
    writes.append((lock_path, text))

    for path, text in writes:
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)
    print(f"set-version: {version} in " + ", ".join(os.path.relpath(p, ROOT) for p, _ in writes))


if __name__ == "__main__":
    try:
        main(sys.argv)
    except Fail as e:
        print(f"set-version: {e}", file=sys.stderr)
        sys.exit(2)
