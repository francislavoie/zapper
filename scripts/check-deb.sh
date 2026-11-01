#!/usr/bin/env bash
# Usage: scripts/check-deb.sh <package.deb> [version]
#
# What a shippable .deb must be, checked before it is released. `version` (the SemVer one, as
# tagged) also checks the control file's Version is its Debian form (scripts/fix-deb.sh).
#
#   - named zapper, with libmpv, WebKitGTK and GTK among its Depends;
#   - /usr/bin/zapper and nothing else in /usr/bin: the harness binaries once shipped there
#     (src-tauri/Cargo.toml, [features]);
#   - /usr/lib/zapper/install-kind saying `deb`: without it the updater thinks the install is a
#     build from source and never offers to update it;
#   - the license notices in /usr/share/doc/zapper/, none empty (docs/licensing.md);
#   - one desktop entry, with StartupWMClass=zapper (the window's app_id, src-tauri/src/lib.rs),
#     valid by desktop-file-validate when that is installed.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
deb=${1:?usage: check-deb.sh <package.deb> [version]}
want=${2:-}
fail() { echo "check-deb: FAIL: $*" >&2; exit 1; }

[ "$(dpkg-deb -f "$deb" Package)" = zapper ] || fail "the package isn't named zapper: $(dpkg-deb -f "$deb" Package)"
version=$(dpkg-deb -f "$deb" Version)
if [ -n "$want" ]; then
  [ "$version" = "$(python3 "$here/version.py" deb "$want")" ] || fail "Version is $version, expected the Debian form of $want (run scripts/fix-deb.sh)"
fi
deps=$(dpkg-deb -f "$deb" Depends)
for d in libmpv libwebkit2gtk-4.1 libgtk-3; do
  grep -q "$d" <<<"$deps" || fail "Depends lacks $d: $deps"
done

contents=$(dpkg-deb -c "$deb" | awk '{ print $NF }' | sed 's|^\./|/|; s|^\([^/]\)|/\1|')
bins=$(grep -E '^/usr/bin/[^/]+$' <<<"$contents" || true)
[ "$bins" = /usr/bin/zapper ] || fail "/usr/bin should hold zapper alone, has: ${bins//$'\n'/ }"

kind=$(dpkg-deb --fsys-tarfile "$deb" | tar -xO --wildcards '*usr/lib/zapper/install-kind' 2>/dev/null) || true
[ "$kind" = deb ] || fail "/usr/lib/zapper/install-kind isn't 'deb': '$kind'"

# The license notices (docs/licensing.md), each there and not empty: scripts/licenses.sh and the
# UI build make them, tauri.conf.json's files map puts them here.
# dpkg-deb lists paths as ./usr/… or usr/… depending on how the archive was
# built; both become /usr/….
sizes=$(dpkg-deb -c "$deb" | awk '{ p = $NF; sub(/^\.\//, "/", p); if (p !~ /^\//) p = "/" p; print $3, p }')
for f in copyright LICENSE MPV-FFMPEG-NOTICE.txt THIRD-PARTY-NOTICES-rust.txt THIRD-PARTY-NOTICES-ui.txt; do
  size=$(awk -v p="/usr/share/doc/zapper/$f" '$2 == p { print $1 }' <<<"$sizes")
  [ -n "$size" ] && [ "$size" -gt 0 ] || fail "/usr/share/doc/zapper/$f is missing or empty"
done

meta=$(mktemp -d); trap 'rm -rf "$meta"' EXIT
dpkg-deb --fsys-tarfile "$deb" | tar -x -C "$meta" --wildcards '*usr/share/applications/*' 2>/dev/null || true
shopt -s nullglob
desktops=("$meta"/usr/share/applications/*.desktop)
[ ${#desktops[@]} = 1 ] || fail "expected one .desktop entry, found ${#desktops[@]}"
grep -qx 'StartupWMClass=zapper' "${desktops[0]}" || fail "the desktop entry lacks StartupWMClass=zapper"
if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "${desktops[0]}" || fail "desktop-file-validate rejects ${desktops[0]##*/}"
fi

echo "check-deb: OK: zapper $version ($(du -h "$deb" | cut -f1)), install-kind deb; Depends: $deps"
