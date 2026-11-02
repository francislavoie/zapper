#!/usr/bin/env bash
# Usage: scripts/check-server-binary.sh <zapper-server-<version>-linux-x86_64> [checks-dir]
#        (`just check-server-binary`)
#
# Checks the static zapper-server for Linux (scripts/build-server-static.sh) where its README
# says it runs, with Docker or Podman and the network to pull the images:
#
#   - here: `file` and `ldd` call it statically linked, it says its version, and --licenses
#     prints its notices (zapper's, FFmpeg's LGPL, musl's, the crates' and the interface's);
#   - in bare debian:stable-slim and alpine containers, with nothing installed: --version and
#     --licenses, then the service started on a scratch home answering GET /api/version and its
#     web interface, the first-time setup (--import-account, --set-password, --network) against
#     it, and a clean stop on SIGTERM;
#   - in an image FROM scratch holding nothing but it: --version and --licenses;
#   - the sample packaging/server/Dockerfile built around it, run as its README says: its
#     non-root user, the setup through `exec`, a clean stop;
#   - with [checks-dir] (build-server-static.sh --with-checks): recording_check, built for the
#     same target, records the fixture streams through it in a debian:stable-slim container
#     with Python and FFmpeg (fake_stream.py, ffprobe): each recording remuxed into a
#     Matroska file that ffprobe reads with its video, audio and clock track.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
fail() { echo "check-server-binary: FAIL: $*" >&2; exit 1; }
[ $# -ge 1 ] || fail "usage: $0 <binary> [checks-dir]"
bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
checks=${2:+$(cd "$2" && pwd)}
[ -x "$bin" ] || fail "no $1"
engine=$(command -v docker || command -v podman) || fail "Docker or Podman is needed"
version=$(basename "$bin")
version=${version#zapper-server-}
version=${version%-linux-x86_64}

echo "== here: $(basename "$bin"), $(du -h "$bin" | cut -f1)"
desc=$(file -b "$bin")
grep -Eq 'static(-pie)? linked|statically linked' <<<"$desc" || fail "file says: $desc"
echo "  file: $desc"
ld=$(ldd "$bin" 2>&1 || true)
grep -Eq 'not a dynamic executable|statically linked' <<<"$ld" || fail "ldd says: $ld"
echo "  ldd: $(head -1 <<<"$ld" | xargs)"
[ "$("$bin" --version)" = "zapper-server $version" ] || fail "--version says '$("$bin" --version)'"
lic=$("$bin" --licenses)
for want in "MIT License" "GNU LESSER GENERAL PUBLIC LICENSE" "FFmpeg" "musl as a whole is licensed" \
  "THIRD-PARTY-NOTICES-rust" "THIRD-PARTY-NOTICES-ui"; do
  grep -q "$want" <<<"$lic" || fail "--licenses has no '$want'"
done
echo "  --version and --licenses ($(wc -l <<<"$lic") lines)"

# The checks inside a container, as sh -s: $1 the binary.
inside=$(cat <<'EOF'
set -eu
z=$1
say() { echo "  $*"; }
die() { echo "  FAIL: $*" >&2; exit 1; }
"$z" --version
"$z" --licenses > /tmp/licenses.txt
grep -q "GNU LESSER GENERAL PUBLIC LICENSE" /tmp/licenses.txt || die "--licenses"
export ZAPPER_HOME=$(mktemp -d)
"$z" --no-sync > "$ZAPPER_HOME/log" 2>&1 &
pid=$!
i=0
until [ "$("$z" --busy 2>/dev/null)" = idle ]; do
  i=$((i + 1)); [ $i -lt 100 ] || { cat "$ZAPPER_HOME/log"; die "it did not answer"; }
  sleep 0.1
done
# Plain HTTP on 127.0.0.1, with whatever the image has: busybox wget, or bash.
get() {
  if command -v wget > /dev/null; then wget -qO- "http://127.0.0.1:8796$1"
  else bash -c "exec 3<>/dev/tcp/127.0.0.1/8796; printf 'GET $1 HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n' >&3; cat <&3"
  fi
}
get /api/version | grep -q "\"version\"" || die "GET /api/version"
get / | grep -qi "<html" || die "GET / is not the web interface"
say "answering /api/version and the web interface"
printf '{"kind":"m3u","playlist":"http://127.0.0.1:9/list.m3u"}\n' | "$z" --import-account - > /dev/null
echo 'a-long-enough-password' | "$z" --set-password > /dev/null
"$z" --network off > /dev/null
say "set up through --import-account, --set-password and --network"
kill -TERM "$pid"
wait "$pid" || die "exited with $? on SIGTERM"
grep -q 'stopped' "$ZAPPER_HOME/log" || { cat "$ZAPPER_HOME/log"; die "no clean stop in its log"; }
say "stopped cleanly on SIGTERM"
EOF
)
for image in debian:stable-slim alpine:3.23; do
  echo "== in a bare $image"
  "$engine" run --rm -i -v "$bin:/zapper-server:ro" "$image" sh -s /zapper-server <<<"$inside"
done

work=$(mktemp -d)
tags=()
cleanup() {
  rm -rf "$work"
  for t in "${tags[@]}"; do "$engine" rmi -f "$t" > /dev/null 2>&1 || true; done
}
trap cleanup EXIT

echo "== FROM scratch"
mkdir "$work/scratch"
cp "$bin" "$work/scratch/zapper-server"
printf 'FROM scratch\nCOPY zapper-server /zapper-server\nENTRYPOINT ["/zapper-server"]\n' > "$work/scratch/Dockerfile"
tag=zapper-server-scratch-check:$$
tags+=("$tag")
"$engine" build -q -t "$tag" "$work/scratch" > /dev/null
[ "$("$engine" run --rm "$tag" --version)" = "zapper-server $version" ] || fail "--version from scratch"
"$engine" run --rm "$tag" --licenses > "$work/scratch/licenses.txt"
grep -q "GNU LESSER GENERAL PUBLIC LICENSE" "$work/scratch/licenses.txt" || fail "--licenses from scratch"
echo "  --version and --licenses with nothing else in the image"

echo "== the sample Dockerfile"
mkdir "$work/image"
cp "$bin" "$work/image/zapper-server"
cp "$root/packaging/server/Dockerfile" "$work/image/Dockerfile"
tag=zapper-server-image-check:$$
tags+=("$tag")
"$engine" build -q -t "$tag" "$work/image" > /dev/null
id=$("$engine" run -d "$tag")
ok=
for _ in $(seq 1 100); do
  if [ "$("$engine" exec "$id" zapper-server --busy 2>/dev/null)" = idle ]; then ok=1; break; fi
  sleep 0.1
done
echo 'a-long-enough-password' | "$engine" exec -i "$id" zapper-server --set-password > /dev/null || ok=
user=$("$engine" inspect --format '{{.Config.User}}' "$tag")
"$engine" stop -t 30 "$id" > /dev/null
log=$("$engine" logs "$id" 2>&1)
"$engine" rm "$id" > /dev/null
[ -n "$ok" ] || fail "the image's service did not answer, or refused the password: $log"
[ -n "$user" ] && [ "$user" != root ] && [ "$user" != 0 ] || fail "the image runs as '$user'"
grep -q 'stopped' <<<"$log" || fail "the image's service did not stop cleanly: $log"
echo "  runs as $user, takes the setup through exec, stops cleanly"

if [ -n "$checks" ]; then
  echo "== recording the fixture streams through it (recording_check)"
  [ -x "$checks/recording_check" ] && [ -x "$checks/zapper-server" ] || fail "no recording_check and zapper-server in $checks"
  cmp -s "$checks/zapper-server" "$bin" || fail "$checks/zapper-server is not $(basename "$bin")"
  # The repository at /src, where recording_check was built: it runs tools/fake_stream.py from
  # there.
  "$engine" run --rm -i -v "$root:/src:ro" -v "$checks:/checks:ro" debian:stable-slim sh -s <<'EOF'
set -eu
apt-get update -qq > /dev/null
# Python and FFmpeg for the fixtures, kill (procps) for its stop part way; the certificate authorities, which the service needs
# to build its HTTP client at all (packaging/server/README.md).
apt-get install -y -qq --no-install-recommends python3 ffmpeg procps ca-certificates > /dev/null 2>&1
export ZAPPER_FIXTURES=/tmp/fixtures
bash /src/tools/make_fixtures.sh > /dev/null
/checks/recording_check
EOF
fi
echo "check-server-binary: PASS"
