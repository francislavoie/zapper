#!/usr/bin/env python3
"""The libmpv manifest: which published build of libmpv zapper links and ships.

    scripts/libmpv-manifest.py write <tag> <owner/repo> <SHA256SUMS> [manifest.json]
        Writes the manifest for a libmpv release from that release's SHA256SUMS: the
        release workflow attaches the result as manifest.json, and the same command updates
        packaging/libmpv/manifest.json from a downloaded SHA256SUMS.
    scripts/libmpv-manifest.py get <platform> [manifest.json]
        Prints "<url> <sha256>" for one platform (scripts/fetch-libmpv.sh), or fails when the
        manifest has no published archive for it.

The tag is libmpv-<mpv version>-ffmpeg-<FFmpeg version>-r<n>; the archives are
libmpv-<platform>.tar.xz (.zip for Windows).
"""

import json
import re
import sys
from pathlib import Path

MANIFEST = Path(__file__).resolve().parent.parent / "packaging" / "libmpv" / "manifest.json"
PLATFORMS = {
    "linux-x86_64": "libmpv-linux-x86_64.tar.xz",
    "windows-x86_64": "libmpv-windows-x86_64.zip",
    "macos-arm64": "libmpv-macos-arm64.tar.xz",
    "macos-x86_64": "libmpv-macos-x86_64.tar.xz",
}
TAG = re.compile(r"^libmpv-(?P<mpv>\d+\.\d+\.\d+)-ffmpeg-(?P<ffmpeg>\d+\.\d+(?:\.\d+)?)-r(?P<rev>\d+)$")


def parse_sums(text):
    sums = {}
    for line in text.splitlines():
        parts = line.split()
        if len(parts) == 2 and re.fullmatch(r"[0-9a-f]{64}", parts[0]):
            sums[parts[1].lstrip("*")] = parts[0]
    return sums


def manifest(tag, repo, sums):
    m = TAG.match(tag)
    if not m:
        raise SystemExit(f"libmpv-manifest: {tag!r} isn't libmpv-<mpv>-ffmpeg-<ffmpeg>-r<n>")
    if not re.fullmatch(r"[\w.-]+/[\w.-]+", repo):
        raise SystemExit(f"libmpv-manifest: {repo!r} isn't owner/repo")
    platforms = {}
    for platform, name in PLATFORMS.items():
        if name in sums:
            platforms[platform] = {
                "url": f"https://github.com/{repo}/releases/download/{tag}/{name}",
                "sha256": sums[name],
            }
    if not platforms:
        raise SystemExit("libmpv-manifest: SHA256SUMS lists none of the archives")
    return {"tag": tag, "mpv": m["mpv"], "ffmpeg": m["ffmpeg"], "platforms": platforms}


def get(platform, path):
    data = json.loads(Path(path).read_text())
    entry = data.get("platforms", {}).get(platform)
    if not entry or not entry.get("sha256"):
        raise SystemExit(
            f"libmpv-manifest: {path} has no published libmpv for {platform} "
            f"(tag {data.get('tag')}). Build one with scripts/build-libmpv.sh and pass it to "
            "scripts/fetch-libmpv.sh --archive, or run the libmpv workflow (docs/licensing.md)."
        )
    if not re.fullmatch(r"[0-9a-f]{64}", entry["sha256"]) or not entry["url"].startswith("https://"):
        raise SystemExit(f"libmpv-manifest: {path}: malformed entry for {platform}")
    return f"{entry['url']} {entry['sha256']}"


def main(argv):
    if len(argv) >= 5 and argv[1] == "write":
        out = manifest(argv[2], argv[3], parse_sums(Path(argv[4]).read_text()))
        text = json.dumps(out, indent=2) + "\n"
        if len(argv) > 5:
            Path(argv[5]).write_text(text)
        else:
            sys.stdout.write(text)
    elif len(argv) >= 3 and argv[1] == "get":
        print(get(argv[2], argv[3] if len(argv) > 3 else MANIFEST))
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main(sys.argv)
