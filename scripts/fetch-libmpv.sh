#!/usr/bin/env bash
# Usage: scripts/fetch-libmpv.sh [--archive <libmpv-<platform>.tar.xz>] [platform]
#
# Puts the libmpv zapper links and ships into src-tauri/lib/<platform>/ (ignored by git): the
# archive packaging/libmpv/manifest.json names for this platform, downloaded and checked against
# the manifest's SHA-256, then unpacked. src-tauri/build.rs links against it and the packages
# carry it (docs/licensing.md). Does nothing when that archive is already unpacked there.
#
# --archive takes one made by scripts/build-libmpv.sh instead of downloading, for a build of
# your own (or before the manifest names a published one).
#
# platform defaults to this machine's: linux-x86_64, macos-arm64 or macos-x86_64. Windows has
# tools/fetch-libmpv.ps1, which also makes the import library MSVC links with.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/.." && pwd)

archive=""
if [ "${1:-}" = --archive ]; then
  archive=${2:?--archive needs a file}
  archive=$(cd "$(dirname "$archive")" && pwd)/$(basename "$archive")
  shift 2
fi
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) native=linux-x86_64 ;;
  Darwin-arm64) native=macos-arm64 ;;
  Darwin-x86_64) native=macos-x86_64 ;;
  *) native="" ;;
esac
platform=${1:-$native}
[ -n "$platform" ] || { echo "fetch-libmpv: no libmpv build for $(uname -s) $(uname -m)" >&2; exit 1; }

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

lib=$repo/src-tauri/lib
dest=$lib/$platform
cache=$lib/.cache
mkdir -p "$cache"

if [ -n "$archive" ]; then
  [ -f "$archive" ] || { echo "fetch-libmpv: no $archive" >&2; exit 1; }
  want=$(sha256 "$archive")
  echo "fetch-libmpv: using $archive (not the manifest's published build)"
else
  read -r url want < <(python3 -I "$here/libmpv-manifest.py" get "$platform")
  if [ -f "$dest/.archive-sha256" ] && [ "$(cat "$dest/.archive-sha256")" = "$want" ]; then
    echo "fetch-libmpv: $platform is up to date in ${dest#"$repo"/}"
    exit 0
  fi
  archive=$cache/${url##*/}
  if [ ! -f "$archive" ] || [ "$(sha256 "$archive")" != "$want" ]; then
    echo "fetch-libmpv: downloading $url"
    curl -fsSL --retry 4 --retry-all-errors -o "$archive.part" "$url"
    mv "$archive.part" "$archive"
  fi
  got=$(sha256 "$archive")
  [ "$got" = "$want" ] || { echo "fetch-libmpv: $archive's SHA-256 is $got, the manifest says $want" >&2; rm -f "$archive"; exit 1; }
fi

# Unpacked beside the old copy, then swapped in, so an interrupted run never leaves half of one.
tmp=$(mktemp -d "$lib/.unpack.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
case $archive in
  *.zip) python3 -I -m zipfile -e "$archive" "$tmp" ;;
  *) tar -xf "$archive" -C "$tmp" ;;
esac
top=$tmp/libmpv-$platform
[ -d "$top" ] || { echo "fetch-libmpv: $archive has no libmpv-$platform/ inside" >&2; exit 1; }
echo "$want" > "$top/.archive-sha256"
rm -rf "$dest"
mv "$top" "$dest"
echo "fetch-libmpv: libmpv for $platform in ${dest#"$repo"/}"
grep -E '^source (mpv|ffmpeg) ' "$dest/BUILDINFO.txt" | awk '{print "  " $2 " " $3}'
