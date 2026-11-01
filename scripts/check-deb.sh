#!/usr/bin/env bash
# Usage: scripts/check-deb.sh <package.deb> [version]
#
# What a shippable .deb must be, checked before it is released. `version` (the SemVer one, as
# tagged) also checks the control file's Version is its Debian form (scripts/fix-deb.sh).
#
#   - named zapper, with WebKitGTK and GTK among its Depends and no libmpv;
#   - /usr/bin/zapper and /usr/bin/zapper-server (the recording service) and nothing else in
#     /usr/bin: the harness binaries once shipped there (src-tauri/Cargo.toml, [features]);
#   - the bundled libmpv and FFmpeg in /usr/lib/zapper, zapper's own LGPL build, what both
#     programs load (through their RUNPATH), with everything they link from the system in
#     Depends;
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
for d in libwebkit2gtk-4.1 libgtk-3; do
  grep -q "$d" <<<"$deps" || fail "Depends lacks $d: $deps"
done
! grep -q mpv <<<"$deps" || fail "Depends names a libmpv; the package carries its own: $deps"

contents=$(dpkg-deb -c "$deb" | awk '{ print $NF }' | sed 's|^\./|/|; s|^\([^/]\)|/\1|')
bins=$(grep -E '^/usr/bin/[^/]+$' <<<"$contents" | sort || true)
[ "$bins" = $'/usr/bin/zapper\n/usr/bin/zapper-server' ] ||
  fail "/usr/bin should hold zapper and zapper-server alone, has: ${bins//$'\n'/ }"

# The bundled libmpv and FFmpeg (docs/licensing.md): each library there under its soname and
# finding the others beside it (a RUNPATH of $ORIGIN); both programs linking what they need from
# there (a RUNPATH of $ORIGIN/../lib/zapper, and nothing from the build machine); everything
# any of them links that is mpv's or FFmpeg's resolved within the package, none of it a GPL
# library; zapper's build rather than a distribution's (whose libmpv links libplacebo and libass,
# which zapper's has inside); and everything they link from the system named in Depends.
root=$(mktemp -d); trap 'rm -rf "$root"' EXIT
dpkg-deb -x "$deb" "$root"
lib=$root/usr/lib/zapper/libmpv.so.2
[ -s "$lib" ] && [ ! -L "$lib" ] || fail "/usr/lib/zapper/libmpv.so.2 is missing"
dyn() { readelf -d "$1" | sed -n "s/.*($2).*\[\(.*\)\]/\1/p"; }
dyn "$root/usr/bin/zapper" NEEDED | grep -qx libmpv.so.2 || fail "/usr/bin/zapper doesn't link libmpv.so.2"
[ -x "$root/usr/bin/zapper-server" ] || fail "/usr/bin/zapper-server isn't executable"
dyn "$root/usr/bin/zapper-server" NEEDED | grep -q '^libavformat\.so\.' || fail "/usr/bin/zapper-server doesn't link libavformat"
for b in zapper zapper-server; do
  runpath=$(dyn "$root/usr/bin/$b" RUNPATH; dyn "$root/usr/bin/$b" RPATH)
  [ "$runpath" = '$ORIGIN/../lib/zapper' ] || fail "/usr/bin/$b's RUNPATH is '$runpath', not \$ORIGIN/../lib/zapper"
done
bundled=("$root"/usr/lib/zapper/*.so.*)
for f in "${bundled[@]}"; do
  n=${f##*/}
  [ "$(dyn "$f" SONAME)" = "$n" ] || fail "/usr/lib/zapper/$n's SONAME is '$(dyn "$f" SONAME)'"
  case $n in libmpv*|libav*|libsw*)
    [ "$(dyn "$f" RUNPATH; dyn "$f" RPATH)" = '$ORIGIN' ] || fail "/usr/lib/zapper/$n doesn't find its neighbours (RUNPATH \$ORIGIN)" ;;
  esac
done
for f in "$root/usr/bin/zapper" "$root/usr/bin/zapper-server" "${bundled[@]}"; do
  for needed in $(dyn "$f" NEEDED); do
    case $needed in
      libpostproc*|libx264*|libx265*|libxvidcore*|librubberband*|libvidstab*|libzvbi*)
        fail "${f#"$root"} links $needed, which is GPL" ;;
      libplacebo*|libass*) fail "${f#"$root"} links $needed: a distribution's build, not zapper's" ;;
      libmpv*|libav*|libsw*)
        [ -f "$root/usr/lib/zapper/$needed" ] || fail "${f#"$root"} links $needed, which the package lacks" ;;
    esac
  done
done
if command -v dpkg-shlibdeps >/dev/null; then
  # dpkg-shlibdeps maps what they link to this machine's packages; each of those must be in
  # Depends (as an alternative, at least). The bundled libraries belong to no package, hence
  # --ignore-missing-info; it wants a debian/control to exist.
  mkdir -p "$root/.shlibs/debian"
  printf 'Source: zapper\n\nPackage: zapper\nArchitecture: any\n' > "$root/.shlibs/debian/control"
  # Its complaints go to a file and are shown if it fails: a library it can't find (one this
  # machine doesn't have installed) is otherwise a bare exit status with no reason.
  shlibs_err="$root/.shlibs/stderr"
  shlibs_out=$(cd "$root/.shlibs" && dpkg-shlibdeps -O --ignore-missing-info -l"$root/usr/lib/zapper" \
    -e"$root/usr/bin/zapper-server" "${bundled[@]/#/-e}" 2>"$shlibs_err") ||
    fail "dpkg-shlibdeps failed (are the Depends' libraries installed here?): $(tr '\n' ' ' < "$shlibs_err")"
  shlibs=$(sed -n 's/^shlibs:Depends=//p' <<<"$shlibs_out")
  names() { tr ',|' '\n\n' | sed 's/(.*//; s/ //g' | sort -u; }
  named=$(names <<<"$deps")
  # It may map a bundled library to the system's package of the same soname (libavformat.so.62
  # to libavformat62, by Debian's naming rule): those are the package's own, not dependencies.
  own=" $(for f in "${bundled[@]}"; do n=${f##*/}; stem=${n%.so.*}; major=${n##*.so.}
    case $stem in *[0-9]) echo "$stem-$major" ;; *) echo "$stem$major" ;; esac; done | tr '\n' ' ')"
  missing=""
  for p in $(names <<<"$shlibs"); do
    case $p in libc6|"") continue ;; esac
    case $own in *" ${p%t64} "*) continue ;; esac
    grep -qxF "$p" <<<"$named" || missing="$missing $p"
  done
  [ -z "$missing" ] || fail "Depends lacks what the bundled libraries link:$missing"
fi

kind=$(dpkg-deb --fsys-tarfile "$deb" | tar -xO --wildcards '*usr/lib/zapper/install-kind' 2>/dev/null) || true
[ "$kind" = deb ] || fail "/usr/lib/zapper/install-kind isn't 'deb': '$kind'"

# The license notices (docs/licensing.md), each there and not empty: scripts/licenses.sh and the
# UI build make them, tauri.conf.json's files map puts them here.
# dpkg-deb lists paths as ./usr/… or usr/… depending on how the archive was
# built; both become /usr/….
sizes=$(dpkg-deb -c "$deb" | awk '{ p = $NF; sub(/^\.\//, "/", p); if (p !~ /^\//) p = "/" p; print $3, p }')
for f in copyright LICENSE MPV-FFMPEG-NOTICE.txt THIRD-PARTY-NOTICES-libmpv.txt THIRD-PARTY-NOTICES-rust.txt \
         THIRD-PARTY-NOTICES-ui.txt; do
  size=$(awk -v p="/usr/share/doc/zapper/$f" '$2 == p { print $1 }' <<<"$sizes")
  [ -n "$size" ] && [ "$size" -gt 0 ] || fail "/usr/share/doc/zapper/$f is missing or empty"
done

shopt -s nullglob
desktops=("$root"/usr/share/applications/*.desktop)
[ ${#desktops[@]} = 1 ] || fail "expected one .desktop entry, found ${#desktops[@]}"
grep -qx 'StartupWMClass=zapper' "${desktops[0]}" || fail "the desktop entry lacks StartupWMClass=zapper"
if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "${desktops[0]}" || fail "desktop-file-validate rejects ${desktops[0]##*/}"
fi

echo "check-deb: OK: zapper $version ($(du -h "$deb" | cut -f1)), install-kind deb; Depends: $deps"
