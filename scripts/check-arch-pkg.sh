#!/usr/bin/env bash
# Usage: scripts/check-arch-pkg.sh <zapper-*.pkg.tar.zst>
#
# Installs the Arch package in a throwaway archlinux:latest container (Docker and the network
# needed, so `just package` doesn't run it) and checks that:
#   - pacman accepts it and every dependency resolves from Arch's repos;
#   - `pacman -Qkk zapper` finds the .MTREE and no altered files;
#   - /usr/lib/zapper/install-kind says arch;
#   - the license notices are in /usr/share/licenses/zapper/ and not empty;
#   - the desktop entry validates;
#   - ldd finds every library of both programs (zapper, zapper-server) and of the libraries the
#     package bundles;
#   - the app starts headless (xvfb-run, as a normal user, muted, with a scratch home) and is
#     still running after 20 s, without a missing library.
# The container is removed afterwards (--rm, and on interrupt); no other image or container is
# touched.
set -euo pipefail
pkg=$(realpath "${1:?usage: check-arch-pkg.sh <zapper-*.pkg.tar.zst>}")
[ -f "$pkg" ] || { echo "check-arch-pkg: no such file: $pkg" >&2; exit 1; }
command -v docker >/dev/null || { echo "check-arch-pkg: needs Docker" >&2; exit 1; }
name="zapper-arch-check-$$"
trap 'docker rm -f "$name" >/dev/null 2>&1 || true' EXIT INT TERM
docker run --rm --name "$name" -v "$pkg:/pkg/$(basename "$pkg"):ro" -e PKG="/pkg/$(basename "$pkg")" \
  archlinux:latest bash -c '
set -euo pipefail
fail() { echo "check-arch-pkg: FAIL: $*" >&2; exit 1; }
step() { echo; echo "== $*"; }
step "pacman -Syu (refresh, and no partial upgrade against the image)"
pacman -Syu --noconfirm --noprogressbar >/tmp/syu.log 2>&1 || { cat /tmp/syu.log; fail "pacman -Syu"; }
# The image keeps itself small with NoExtract rules (usr/share/doc, man pages, locales) that a
# stock Arch install does not have: they would skip the license notices in /usr/share/doc/zapper
# and make pacman -Qkk report them missing. Install the way a real system does.
sed -i "/^NoExtract/d" /etc/pacman.conf
step "pacman -U $PKG"
pacman -U --noconfirm --noprogressbar "$PKG" 2>&1 | tee /tmp/u.log
grep -Ei "^(warning|error):|missing|corrupt|invalid" /tmp/u.log && fail "pacman -U warned"
step "pacman -Qi zapper"
pacman -Qi zapper | grep -E "^(Name|Version|Description|Architecture|URL|Licenses|Depends On|Installed Size|Packager|Build Date) "
step "pacman -Qkk zapper"
pacman -Qkk zapper 2>&1 | tee /tmp/qkk.log || fail "pacman -Qkk reported problems"
grep -qE "^zapper: [0-9]+ total files, 0 altered files$" /tmp/qkk.log || fail "pacman -Qkk found altered files"
grep -Ei "^(warning|error):|mtree" /tmp/qkk.log && fail "pacman -Qkk warned"
step "install-kind"
[ "$(cat /usr/lib/zapper/install-kind)" = arch ] || fail "/usr/lib/zapper/install-kind is not arch"
step "license notices in /usr/share/licenses/zapper"
for f in LICENSE MPV-FFMPEG-NOTICE.txt THIRD-PARTY-NOTICES-rust.txt THIRD-PARTY-NOTICES-ui.txt; do
  [ -s "/usr/share/licenses/zapper/$f" ] || fail "no /usr/share/licenses/zapper/$f (or it is empty)"
done
ls -lL /usr/share/licenses/zapper/
step "desktop entry"
pacman -S --noconfirm --noprogressbar --needed desktop-file-utils >/dev/null 2>&1 || fail "installing desktop-file-utils"
desktop-file-validate /usr/share/applications/zapper.desktop || fail "desktop-file-validate rejects zapper.desktop"
step "ldd"
missing=0
shopt -s nullglob
for f in /usr/bin/zapper /usr/bin/zapper-server /usr/lib/zapper/*.so*; do
  out=$(ldd "$f"); n=$(grep -c "not found" <<<"$out" || true)
  echo "$f: $(grep -c "=>" <<<"$out") libraries, $n not found"
  [ "$n" = 0 ] || { grep "not found" <<<"$out"; missing=1; }
done
[ "$missing" = 0 ] || fail "ldd: missing libraries"
step "headless launch (xvfb-run, 20 s, as a normal user)"
pacman -S --noconfirm --noprogressbar --needed xorg-server-xvfb xorg-xauth >/dev/null 2>&1 || fail "installing xvfb"
useradd -m tester
set +e
# ZAPPER_MUTE: no audio output; ZAPPER_HOME: a scratch profile (there is no account in here anyway).
runuser -u tester -- env HOME=/home/tester ZAPPER_MUTE=1 ZAPPER_HOME=/home/tester/zapper-home \
  timeout 20 xvfb-run -a zapper >/tmp/run.log 2>&1
rc=$?
set -e
tail -n 20 /tmp/run.log
echo "exit status: $rc (124 = still running when the 20 s timeout stopped it)"
grep -Ei "error while loading shared libraries|cannot open shared object" /tmp/run.log && fail "launch: missing library"
[ "$rc" = 124 ] || fail "the app exited on its own (status $rc) within 20 s"
echo; echo "check-arch-pkg: OK"
'
