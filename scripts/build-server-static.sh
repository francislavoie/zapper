#!/usr/bin/env bash
# Usage: scripts/build-server-static.sh <out-dir> [--with-checks]   (`just package-server`)
#
# zapper-server for Linux as one static file, <out-dir>/zapper-server-<version>-linux-x86_64:
# the recording service built for x86_64-unknown-linux-musl, with nothing to load at run time
# (no glibc, no libraries beside it), for any x86-64 Linux, a container from scratch included.
#
# Built in a pinned Rust image on Alpine (musl is its C library), with Docker or Podman:
#   1. scripts/build-ffmpeg-static.sh: the minimal static LGPL FFmpeg it links;
#   2. cargo build --release for the `server` feature, with ZAPPER_FFMPEG_STATIC naming that
#      FFmpeg and ZAPPER_SERVER_NOTICES the licence notices it carries (`--licenses`): zapper's
#      LICENSE, FFmpeg's notice, musl's COPYRIGHT, the Rust crates' and the interface's notices.
# With --with-checks, also: the browser feed's tests (recording::livemux, which transcode an
# AC-3 fixture to AAC) run against that same static FFmpeg, here in the image, with Alpine's
# ffmpeg command making the fixture; and recording_check (the `harness` feature) for the same
# target, beside it in <out-dir>/checks/, for scripts/check-server-binary.sh to record through.
#
# Needs, from the host: the interface built (`npm run build`, compiled in; its notices in
# dist/licenses) and the Rust notices (scripts/licenses.sh). The FFmpeg build, cargo's
# downloads and its target folder are kept in ZAPPER_STATIC_CACHE (default
# src-tauri/target/static), so a second run rebuilds only what changed.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
die() { echo "build-server-static: $*" >&2; exit 1; }
[ $# -ge 1 ] || die "usage: $0 <out-dir> [--with-checks]"
out=$1
checks=
[ "${2:-}" = --with-checks ] && checks=1
# Pinned, and more than two weeks old when it was picked: the toolchain and C library that make
# the binary.
image=rust:1.98.1-alpine3.23@sha256:94a43bda4fd9b71fa3a74621c0982a646f83c7d1a3ba52bb4b7ddfd76d4dca93
engine=$(command -v docker || command -v podman) || die "Docker or Podman is needed"
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/src-tauri/Cargo.toml" | head -1)
cache=${ZAPPER_STATIC_CACHE:-$root/src-tauri/target/static}
notices_ui=$root/dist/licenses/THIRD-PARTY-NOTICES-ui.txt
notices_rust=$root/src-tauri/target/licenses/THIRD-PARTY-NOTICES-rust.txt
[ -f "$root/dist/index.html" ] || die "no dist/index.html: run npm run build first"
for f in "$notices_ui" "$notices_rust"; do
  [ -s "$f" ] || die "$f is missing or empty (npm run build, scripts/licenses.sh)"
done
mkdir -p "$cache/notices" "$out"
# The notices, in the order --licenses prints them.
cp "$root/LICENSE" "$cache/notices/1-LICENSE.txt"
cp "$notices_rust" "$cache/notices/4-THIRD-PARTY-NOTICES-rust.txt"
cp "$notices_ui" "$cache/notices/5-THIRD-PARTY-NOTICES-ui.txt"
cp "$root/packaging/licenses/MUSL-COPYRIGHT.txt" "$cache/notices/3-MUSL-COPYRIGHT.txt"

harness=
[ -n "$checks" ] && harness='apk add --no-cache -q ffmpeg
cargo test --release --locked --manifest-path /src/src-tauri/Cargo.toml --no-default-features --features server --lib recording::livemux -- --nocapture
cargo build --release --locked --manifest-path /src/src-tauri/Cargo.toml --no-default-features --features server,harness --bin recording_check'

"$engine" run --rm -i \
  -v "$root:/src" -v "$cache:/cache" \
  -e CARGO_HOME=/cache/cargo -e CARGO_TARGET_DIR=/cache/target \
  -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(nproc)}" -e JOBS="${CARGO_BUILD_JOBS:-$(nproc)}" \
  -e CARGO_PROFILE_RELEASE_STRIP=symbols \
  -e ZAPPER_FFMPEG_STATIC=/cache/ffmpeg -e ZAPPER_SERVER_NOTICES=/cache/notices \
  -e OWNER="$(id -u):$(id -g)" -e HARNESS="$harness" \
  "$image" sh -s <<'EOF'
set -eu
apk add --no-cache -q bash build-base nasm perl xz curl cmake linux-headers file
trap 'chown -R "$OWNER" /cache' EXIT
bash /src/scripts/build-ffmpeg-static.sh /cache/ffmpeg /cache/sources
cp /cache/ffmpeg/FFMPEG-NOTICE.txt /cache/notices/2-FFMPEG-NOTICE.txt
cargo build --release --locked --manifest-path /src/src-tauri/Cargo.toml \
  --no-default-features --features server --bin zapper-server
file /cache/target/release/zapper-server
if [ -n "$HARNESS" ]; then sh -ec "$HARNESS"; fi
EOF

bin=$out/zapper-server-$version-linux-x86_64
install -m 0755 "$cache/target/release/zapper-server" "$bin"
if [ -n "$checks" ]; then
  mkdir -p "$out/checks"
  install -m 0755 "$cache/target/release/recording_check" "$out/checks/recording_check"
  # recording_check runs the zapper-server beside it.
  install -m 0755 "$bin" "$out/checks/zapper-server"
fi
echo "build-server-static: $bin ($(du -h "$bin" | cut -f1))"
file "$bin"
