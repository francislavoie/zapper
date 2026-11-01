#!/usr/bin/env bash
# Usage: scripts/check-macos-app.sh <zapper.app>
#
# Checks the macOS bundle `just package-macos` (packaging/macos/dmg.conf.json) makes, the way
# scripts/check-deb.sh checks the .deb:
#
# - every Mach-O in Contents/MacOS and Contents/Frameworks links only the system's libraries and
#   frameworks and the dylibs bundled beside it, each @rpath one resolving through the binary's
#   own rpaths to a file in the bundle, and no rpath names a directory of the build machine;
# - nothing linked is under the GPL: mpv's own feature list has no `gpl`, and FFmpeg reports
#   itself LGPL-2.1-or-later, configured without --enable-gpl, --enable-version3 or
#   --enable-nonfree;
# - the license notices and the install-kind marker are in Contents/Resources;
# - the bundle's signature (ad hoc) verifies.
set -euo pipefail

app=${1:?usage: check-macos-app.sh <zapper.app>}
[ -d "$app/Contents" ] || { echo "check-macos-app: $app is not an app bundle" >&2; exit 2; }
app=$(cd "$app" && pwd)
fail=0
bad() { echo "check-macos-app: $*" >&2; fail=1; }

macho() { file -b "$1" | grep -q '^Mach-O'; }

# The rpaths a binary carries (LC_RPATH), one per line.
rpaths() { otool -l "$1" | awk '$1 == "cmd" && $2 == "LC_RPATH" { getline; getline; print $2 }'; }

binaries=()
while IFS= read -r -d '' f; do
  macho "$f" && binaries+=("$f")
done < <(find "$app/Contents/MacOS" "$app/Contents/Frameworks" -type f -print0 2>/dev/null)
[ ${#binaries[@]} -gt 0 ] || bad "no binaries in $app"

main=$app/Contents/MacOS/zapper
[ -x "$main" ] || bad "no Contents/MacOS/zapper"
for lib in libmpv.2.dylib libavutil.60.dylib libavcodec.62.dylib libavformat.62.dylib \
    libavfilter.11.dylib libavdevice.62.dylib libswscale.9.dylib libswresample.6.dylib; do
  [ -f "$app/Contents/Frameworks/$lib" ] || bad "Contents/Frameworks/$lib is missing"
done

for f in "${binaries[@]}"; do
  rel=${f#"$app"/}
  dir=$(dirname "$f")
  rps=$(rpaths "$f")
  for rp in $rps; do
    case $rp in
      @executable_path/*|@loader_path|@loader_path/*) ;;
      # The Swift runtime's home in macOS itself, which libmpv's Swift parts name.
      /usr/lib/swift) ;;
      *) bad "$rel has an rpath outside the bundle: $rp" ;;
    esac
  done
  # The first line is the file name; a dylib's second is its own install name.
  deps=$(otool -L "$f" | tail -n +2 | awk '{print $1}')
  case $f in *.dylib) deps=$(echo "$deps" | tail -n +2) ;; esac
  for dep in $deps; do
    case $dep in
      /usr/lib/*|/System/Library/*) ;;
      @rpath/*)
        name=${dep#@rpath/}
        found=""
        for rp in $rps; do
          d=${rp/#@executable_path/$app/Contents/MacOS}
          d=${d/#@loader_path/$dir}
          [ -f "$d/$name" ] && { found=$d/$name; break; }
        done
        if [ -z "$found" ]; then
          bad "$rel links $dep, which none of its rpaths ($(echo ${rps:-none})) finds in the bundle"
        fi
        ;;
      *) bad "$rel links $dep, which is neither the system's nor in the bundle" ;;
    esac
  done
  echo "ok  $rel: $(echo $deps | wc -w | tr -d ' ') libraries, rpaths: $(echo ${rps:-none})"
done

# mpv's feature list is one sorted, space-separated string compiled into the library; a GPL
# build has `gpl` among them.
features=$(strings -a "$app/Contents/Frameworks/libmpv.2.dylib" |
  grep -E '(^| )videotoolbox-gl( |$)' | grep -E '(^| )libplacebo( |$)' | head -1 || true)
if [ -z "$features" ]; then
  bad "libmpv.2.dylib has no feature list to check"
elif echo " $features " | grep -q ' gpl '; then
  bad "libmpv.2.dylib is a GPL build: $features"
else
  echo "ok  mpv features: $features"
fi
# FFmpeg's license and configure line, as libavutil carries them.
avutil=$app/Contents/Frameworks/libavutil.60.dylib
strings -a "$avutil" | grep -qx 'LGPL version 2.1 or later' || bad "libavutil does not say LGPL version 2.1 or later"
config=$(strings -a "$avutil" | grep -E -- '--prefix=' | head -1 || true)
for flag in --enable-gpl --enable-version3 --enable-nonfree; do
  case " $config " in *" $flag "*) bad "FFmpeg was configured with $flag" ;; esac
done
echo "ok  FFmpeg: LGPL version 2.1 or later"

res=$app/Contents/Resources
for n in LICENSE.txt MPV-FFMPEG-NOTICE.txt THIRD-PARTY-NOTICES-libmpv.txt THIRD-PARTY-NOTICES-rust.txt \
    THIRD-PARTY-NOTICES-ui.txt; do
  [ -s "$res/licenses/$n" ] || bad "Contents/Resources/licenses/$n is missing or empty"
done
[ "$(cat "$res/install-kind" 2>/dev/null)" = dmg ] || bad "Contents/Resources/install-kind does not say dmg"

codesign --verify --deep --strict --verbose=2 "$app" || bad "the signature does not verify"
codesign -dv "$app" 2>&1 | grep -E '^(Signature|Identifier|Format)' || true

[ $fail = 0 ] || exit 1
echo "check-macos-app: $app is fine"
