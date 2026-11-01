#!/usr/bin/env bash
# Usage: scripts/test-scripts.sh  (`just test-scripts`)
#
# The release scripts, on throwaway copies: the version rules and their order, the changelog's
# release and notes, set-version on copies of the real files (only the versions change), the
# placeholder repository refused by release.sh, fix-deb/check-deb on a small fake .deb, the Arch
# package made from it (package-arch.sh and arch-pkg.py: no pacman needed), and the license
# helpers in notices.py.
# Nothing here touches the repository's own files, its tags or the network.
set -euo pipefail
# Package modes as a build machine makes them, whatever this shell's umask.
umask 022
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
n=0
ok() { n=$((n + 1)); }
fail() { echo "test-scripts: FAIL: $*" >&2; exit 1; }
v() { python3 "$here/version.py" "$@"; }

# --- version.py
for good in 0.1.0 1.2.3 0.2.0-rc.1 0.2.0-alpha.1.2 10.0.0-beta; do
  v check "$good" || fail "$good should be a release version"; ok
done
for bad in v0.1.0 0.1 01.2.3 0.1.0+abc 0.1.0-RC.1 0.1.0-alpha10 0.1.0-1 ""; do
  ! v check "$bad" 2>/dev/null || fail "'$bad' should be refused"; ok
done
order=(0.1.0-alpha.1 0.1.0-alpha.2 0.1.0-beta 0.1.0-rc.1 0.1.0 0.1.1 0.2.0-rc.1 0.2.0 0.10.0 1.0.0)
for ((i = 1; i < ${#order[@]}; i++)); do
  [ "$(v cmp "${order[i-1]}" "${order[i]}")" = -1 ] || fail "${order[i-1]} < ${order[i]}"; ok
  # dpkg must agree with SemVer once fix-deb has rewritten the version.
  if command -v dpkg >/dev/null; then
    dpkg --compare-versions "$(v deb "${order[i-1]}")" lt "$(v deb "${order[i]}")" || fail "dpkg: ${order[i-1]} < ${order[i]}"; ok
  fi
done
[ "$(v deb 0.2.0-rc.1)" = 0.2.0~rc.1 ] && [ "$(v deb 0.2.0)" = 0.2.0 ] && [ "$(v deb 0.2.0~rc.1)" = 0.2.0~rc.1 ] || fail "deb form"; ok
[ "$(v prerelease 0.2.0-rc.1)" = true ] && [ "$(v prerelease 0.2.0)" = false ] || fail "prerelease"; ok

# --- changelog.py
cl=$tmp/CHANGELOG.md
printf '# Changelog\n\n## [Unreleased]\n\n### Added\n- A thing.\n\n## [0.1.0] - 2026-10-01\n\n- First.\n' > "$cl"
python3 "$here/changelog.py" release 0.2.0 2026-10-31 "$cl"
grep -qx '## \[0.2.0\] - 2026-10-31' "$cl" || fail "changelog release: no dated section"; ok
grep -qx '## \[Unreleased\]' "$cl" || fail "changelog release: no fresh Unreleased"; ok
[ "$(python3 "$here/changelog.py" notes 0.2.0 "$cl")" = $'### Added\n- A thing.' ] || fail "changelog notes"; ok
! python3 "$here/changelog.py" release 0.3.0 2026-11-01 "$cl" 2>/dev/null || fail "an empty Unreleased must be refused"; ok
! python3 "$here/changelog.py" notes 9.9.9 "$cl" 2>/dev/null || fail "a missing section must be refused"; ok
python3 "$here/changelog.py" notes Unreleased "$root/CHANGELOG.md" >/dev/null || fail "CHANGELOG.md's Unreleased section is empty"; ok

# --- set-version.py, on copies laid out like the repository
copy=$tmp/repo
mkdir -p "$copy/scripts" "$copy/src-tauri"
cp "$here/set-version.py" "$copy/scripts/"
cp "$root/package.json" "$root/package-lock.json" "$copy/"
cp "$root/src-tauri/tauri.conf.json" "$root/src-tauri/Cargo.toml" "$root/src-tauri/Cargo.lock" "$copy/src-tauri/"
python3 "$copy/scripts/set-version.py" 9.8.7-rc.1 >/dev/null
[ "$(jq -r .version "$copy/src-tauri/tauri.conf.json")" = 9.8.7-rc.1 ] || fail "tauri.conf.json"; ok
[ "$(jq -r .version "$copy/package.json")" = 9.8.7-rc.1 ] || fail "package.json"; ok
[ "$(jq -r '.version + " " + .packages[""].version' "$copy/package-lock.json")" = "9.8.7-rc.1 9.8.7-rc.1" ] || fail "package-lock.json"; ok
grep -q '^version = "9.8.7-rc.1"$' "$copy/src-tauri/Cargo.toml" || fail "Cargo.toml"; ok
[ "$(grep -A1 '^name = "zapper"$' "$copy/src-tauri/Cargo.lock" | tail -1)" = 'version = "9.8.7-rc.1"' ] || fail "Cargo.lock"; ok
for f in package.json package-lock.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock; do
  changed=$(diff "$root/$f" "$copy/$f" | grep -c '^>' || true)
  [ "$changed" -ge 1 ] && [ "$changed" -le 2 ] || fail "$f: $changed lines changed, expected only the version"; ok
done
! python3 "$copy/scripts/set-version.py" '1.0.0"; rm' 2>/dev/null || fail "set-version must refuse odd characters"; ok

# --- releases-repo.sh, and release.sh refusing the placeholder before doing anything
repo=$("$here/releases-repo.sh")
[ -n "$repo" ] || fail "releases-repo.sh"; ok
case "$repo" in
  OWNER/*)
    out=$("$here/release.sh" 0.0.1 2>&1) && fail "release.sh must refuse the placeholder repository"
    grep -q placeholder <<<"$out" || fail "release.sh's refusal should name the placeholder: $out"; ok ;;
esac

# --- fix-deb.sh and check-deb.sh, on a tiny .deb shaped like Tauri's
if command -v dpkg-deb >/dev/null && command -v cc >/dev/null; then
  pkg=$tmp/pkg
  mkdir -p "$pkg/DEBIAN" "$pkg/usr/bin" "$pkg/usr/lib/zapper" "$pkg/usr/share/applications" "$pkg/usr/share/doc/zapper"
  for f in copyright LICENSE MPV-FFMPEG-NOTICE.txt; do
    printf 'notice\n' > "$pkg/usr/share/doc/zapper/$f"
  done
  # The third-party notices open with a summary of their licenses (the Arch package's license field).
  printf 'Rust notices\n\nSummary\n-------\n  MIT          40\n  Apache-2.0    2\n  LGPL-2.1      1\n\n\nMIT\n' \
    > "$pkg/usr/share/doc/zapper/THIRD-PARTY-NOTICES-rust.txt"
  ui_notices='UI notices\n\nSummary\n-------\n  MIT     9\n  ISC     3\n\n\nMIT\n'
  printf "$ui_notices" > "$pkg/usr/share/doc/zapper/THIRD-PARTY-NOTICES-ui.txt"
  printf 'libmpv notices\n\nSummary\n-------\n  LGPL-2.1-or-later   4\n  MIT                 3\n\n\nmpv\n' \
    > "$pkg/usr/share/doc/zapper/THIRD-PARTY-NOTICES-libmpv.txt"
  # Programs and libraries shaped like the real ones: zapper links libmpv.so.2, zapper-server
  # libavformat.so.62, each finding it through $ORIGIN/../lib/zapper; the libraries find each
  # other through $ORIGIN.
  printf 'int mpv_client_api_version(void) { return 0; }\n' > "$tmp/mpv.c"
  cc -shared -fPIC -Wl,-soname,libmpv.so.2 -Wl,-rpath,'$ORIGIN' -o "$pkg/usr/lib/zapper/libmpv.so.2" "$tmp/mpv.c"
  printf 'int avformat_version(void) { return 0; }\n' > "$tmp/avformat.c"
  cc -shared -fPIC -Wl,-soname,libavformat.so.62 -Wl,-rpath,'$ORIGIN' -o "$pkg/usr/lib/zapper/libavformat.so.62" "$tmp/avformat.c"
  printf 'int mpv_client_api_version(void);\nint main(void) { return mpv_client_api_version(); }\n' > "$tmp/zapper.c"
  cc -o "$pkg/usr/bin/zapper" "$tmp/zapper.c" "$pkg/usr/lib/zapper/libmpv.so.2" -Wl,-rpath,'$ORIGIN/../lib/zapper'
  printf 'int avformat_version(void);\nint main(void) { return avformat_version(); }\n' > "$tmp/server.c"
  cc -o "$pkg/usr/bin/zapper-server" "$tmp/server.c" "$pkg/usr/lib/zapper/libavformat.so.62" -Wl,-rpath,'$ORIGIN/../lib/zapper'
  cp "$root/packaging/install-kind-deb" "$pkg/usr/lib/zapper/install-kind"
  printf '[Desktop Entry]\nType=Application\nName=zapper\nExec=zapper\nIcon=zapper\nCategories=AudioVideo;Video;\nStartupWMClass=zapper\n' > "$pkg/usr/share/applications/zapper.desktop"
  printf 'Package: zapper\nVersion: 0.2.0-rc.1\nArchitecture: amd64\nMaintainer: t\nDepends: libwebkit2gtk-4.1-0, libgtk-3-0\nDescription: t\n' > "$pkg/DEBIAN/control"
  dpkg-deb --root-owner-group --build "$pkg" "$tmp/zapper.deb" >/dev/null
  data_before=$(cd "$tmp" && ar p zapper.deb "$(ar t zapper.deb | grep ^data)" | sha256sum)
  ! "$here/check-deb.sh" "$tmp/zapper.deb" 0.2.0-rc.1 >/dev/null 2>&1 || fail "check-deb must refuse the SemVer Version"; ok
  "$here/fix-deb.sh" "$tmp/zapper.deb" >/dev/null
  [ "$(dpkg-deb -f "$tmp/zapper.deb" Version)" = 0.2.0~rc.1 ] || fail "fix-deb's Version"; ok
  data_after=$(cd "$tmp" && ar p zapper.deb "$(ar t zapper.deb | grep ^data)" | sha256sum)
  [ "$data_before" = "$data_after" ] || fail "fix-deb changed the data member"; ok
  "$here/fix-deb.sh" "$tmp/zapper.deb" | grep -q "already" || fail "fix-deb is idempotent"; ok
  "$here/check-deb.sh" "$tmp/zapper.deb" 0.2.0-rc.1 >/dev/null || fail "check-deb on the fixed package"; ok
  printf 'rpm\n' > "$pkg/usr/lib/zapper/install-kind"
  dpkg-deb --root-owner-group --build "$pkg" "$tmp/wrong.deb" >/dev/null
  ! "$here/check-deb.sh" "$tmp/wrong.deb" >/dev/null 2>&1 || fail "check-deb must refuse a .deb whose marker isn't deb"; ok
  printf 'deb\n' > "$pkg/usr/lib/zapper/install-kind"
  : > "$pkg/usr/share/doc/zapper/THIRD-PARTY-NOTICES-ui.txt"
  dpkg-deb --root-owner-group --build "$pkg" "$tmp/nonotice.deb" >/dev/null
  ! "$here/check-deb.sh" "$tmp/nonotice.deb" >/dev/null 2>&1 || fail "check-deb must refuse a .deb with an empty notice"; ok
  printf 'notice\n' > "$pkg/usr/share/doc/zapper/THIRD-PARTY-NOTICES-ui.txt"
  # The system's libmpv in Depends, the bundled one missing, or a binary that would look for it
  # elsewhere: each refused.
  sed -i 's/^Depends: /Depends: libmpv2, /' "$pkg/DEBIAN/control"
  dpkg-deb --root-owner-group --build "$pkg" "$tmp/sysmpv.deb" >/dev/null
  ! "$here/check-deb.sh" "$tmp/sysmpv.deb" >/dev/null 2>&1 || fail "check-deb must refuse a .deb depending on libmpv"; ok
  sed -i 's/^Depends: libmpv2, /Depends: /' "$pkg/DEBIAN/control"
  cc -o "$pkg/usr/bin/zapper" "$tmp/zapper.c" "$pkg/usr/lib/zapper/libmpv.so.2" -Wl,-rpath,/home/builder/lib
  dpkg-deb --root-owner-group --build "$pkg" "$tmp/rpath.deb" >/dev/null
  ! "$here/check-deb.sh" "$tmp/rpath.deb" >/dev/null 2>&1 || fail "check-deb must refuse a binary with a build machine's RUNPATH"; ok
  cc -o "$pkg/usr/bin/zapper" "$tmp/zapper.c" "$pkg/usr/lib/zapper/libmpv.so.2" -Wl,-rpath,'$ORIGIN/../lib/zapper'
  # The server's FFmpeg missing, or the server itself: refused too.
  mv "$pkg/usr/lib/zapper/libavformat.so.62" "$tmp/"
  dpkg-deb --root-owner-group --build "$pkg" "$tmp/noav.deb" >/dev/null
  ! "$here/check-deb.sh" "$tmp/noav.deb" >/dev/null 2>&1 || fail "check-deb must refuse a .deb without the server's FFmpeg"; ok
  mv "$tmp/libavformat.so.62" "$pkg/usr/lib/zapper/"
  mv "$pkg/usr/bin/zapper-server" "$tmp/"
  dpkg-deb --root-owner-group --build "$pkg" "$tmp/noserver.deb" >/dev/null
  ! "$here/check-deb.sh" "$tmp/noserver.deb" >/dev/null 2>&1 || fail "check-deb must refuse a .deb without zapper-server"; ok
  mv "$tmp/zapper-server" "$pkg/usr/bin/"
  rm "$pkg/usr/lib/zapper/libmpv.so.2"
  dpkg-deb --root-owner-group --build "$pkg" "$tmp/nompv.deb" >/dev/null
  ! "$here/check-deb.sh" "$tmp/nompv.deb" >/dev/null 2>&1 || fail "check-deb must refuse a .deb without the bundled libmpv"; ok
fi

# --- arch-pkg.py: the version, the dependency table, bundled libraries, mtree escaping
a() { python3 "$here/arch-pkg.py" "$@"; }
[ "$(a pkgver 0.2.0~rc.1)" = 0.2.0rc.1 ] && [ "$(a pkgver 0.2.0)" = 0.2.0 ] || fail "arch pkgver"; ok
! a pkgver 1:0.2.0 2>/dev/null && ! a pkgver 0.2.0-1 2>/dev/null || fail "arch pkgver must refuse ':' and '-'"; ok
# What tauri.conf.json's Depends becomes, Tauri's repeats included.
[ "$(a depends 'libmpv2 | libmpv1, libwebkit2gtk-4.1-0, libgtk-3-0, libwebkit2gtk-4.1-0, libgtk-3-0' | tr '\n' ' ')" = 'gtk3 mpv webkit2gtk-4.1 ' ] ||
  fail "arch depends"; ok
[ "$(a depends 'libasound2t64 (>= 1.0.17), libc6 (>= 2.39), libglib2.0-0t64' | tr '\n' ' ')" = 'alsa-lib glib2 glibc ' ] ||
  fail "arch depends: versions and t64 names"; ok
[ "$(a depends 'libfoo9 | libmpv1')" = mpv ] || fail "arch depends: the first alternative the table knows"; ok
err=$(a depends 'libgtk-3-0, libfoo9 (>= 1)' 2>&1 >/dev/null) && fail "an unknown dependency must fail"
grep -q 'unknown Debian dependencies.*libfoo9' <<<"$err" || fail "unclear unknown-dependency message: $err"; ok
# A library the package carries is no dependency, known to the table or not.
lib=$tmp/bundled/usr/lib/zapper
mkdir -p "$lib"
: > "$lib/libmpv.so.2.3.0"; : > "$lib/libavcodec.so.61"; : > "$lib/libfoo-1.0.so.0"; : > "$lib/libbar.so"
[ "$(a bundled "$tmp/bundled" | tr '\n' ' ')" = 'libavcodec61 libfoo-1.0-0 libmpv2 ' ] || fail "arch bundled: $(a bundled "$tmp/bundled")"; ok
[ "$(a depends 'libmpv2 | libmpv1, libavcodec61t64 (>= 7), libfoo-1.0-0, libgtk-3-0' "$tmp/bundled" 2>/dev/null)" = gtk3 ] ||
  fail "arch depends: bundled libraries dropped"; ok
a depends 'libmpv2, libgtk-3-0' "$tmp/bundled" 2>&1 >/dev/null | grep -q "dropped 'libmpv2': the package bundles it" ||
  fail "arch depends: a dropped dependency is noted"; ok
[ "$(a escape 'usr/a b#c=d\e')" = 'usr/a\040b\043c\075d\134e' ] && [ "$(a escape 'é')" = '\303\251' ] || fail "arch mtree escaping"; ok

# --- package-arch.sh, on the fixed fake .deb
if command -v dpkg-deb >/dev/null && command -v zstd >/dev/null; then
  printf "$ui_notices" > "$pkg/usr/share/doc/zapper/THIRD-PARTY-NOTICES-ui.txt"
  # The variants below are built from $pkg, so with the Version fix-deb gave zapper.deb: each
  # must be refused for what it changes, not for a SemVer version.
  sed -i 's/^Version: .*/Version: 0.2.0~rc.1/' "$pkg/DEBIAN/control"
  SOURCE_DATE_EPOCH=1791206700 "$here/package-arch.sh" "$tmp/zapper.deb" "$tmp/arch" >/dev/null
  out=$tmp/arch/zapper-0.2.0rc.1-1-x86_64.pkg.tar.zst
  [ -f "$out" ] || fail "package-arch: no $out: $(ls "$tmp/arch")"; ok
  [ -z "$(find "$tmp/arch" -name '.work.*')" ] || fail "package-arch left its work directory"; ok
  zstd -dcq "$out" > "$tmp/p.tar"
  [ "$(tar -tf "$tmp/p.tar" | head -3 | tr '\n' ' ')" = '.PKGINFO .MTREE usr/ ' ] || fail "arch: archive order"; ok
  listing=$(tar -tvf "$tmp/p.tar")
  grep -Eq '^-rwxr-xr-x root/root .* usr/bin/zapper$' <<<"$listing" || fail "arch: usr/bin/zapper"; ok
  grep -Eq '^lrwxrwxrwx root/root .* usr/share/licenses/zapper -> \.\./doc/zapper$' <<<"$listing" || fail "arch: licenses symlink"; ok
  ! grep -v ' root/root ' <<<"$listing" | grep -q . || fail "arch: an entry isn't root:root"; ok
  [ "$(tar -xOf "$tmp/p.tar" usr/lib/zapper/install-kind)" = arch ] || fail "arch: install-kind"; ok
  info=$(tar -xOf "$tmp/p.tar" .PKGINFO)
  for kv in 'pkgname = zapper' 'pkgver = 0.2.0rc.1-1' 'pkgdesc = t' 'arch = x86_64' 'builddate = 1791206700' \
            "url = https://github.com/$repo" 'depend = gtk3' 'depend = webkit2gtk-4.1'; do
    grep -qxF "$kv" <<<"$info" || fail ".PKGINFO lacks '$kv': $info"; ok
  done
  ! grep -q '^depend = mpv' <<<"$info" || fail "arch: depends on mpv, though the package carries libmpv: $info"; ok
  [ "$(sed -n 's/^license = //p' <<<"$info" | tr '\n' ' ')" = 'MIT Apache-2.0 ISC LGPL-2.1 LGPL-2.1-or-later ' ] ||
    fail "arch: licenses: $info"; ok
  mtree=$(tar -xOf "$tmp/p.tar" .MTREE | gzip -dc)
  [ "$(head -2 <<<"$mtree" | tr '\n' '|')" = '#mtree|/set type=file uid=0 gid=0 mode=644|' ] || fail "arch: .MTREE header"; ok
  grep -Eq '^\./\.PKGINFO time=1791206700\.0 size=[0-9]+ md5digest=[0-9a-f]{32} sha256digest=[0-9a-f]{64}$' <<<"$mtree" || fail "arch: .MTREE .PKGINFO: $mtree"; ok
  grep -Eq '^\./usr/bin/zapper time=1791206700\.0 mode=755 size=[0-9]+ ' <<<"$mtree" || fail "arch: .MTREE binary: $mtree"; ok
  grep -qxF './usr/share/licenses/zapper time=1791206700.0 mode=777 type=link link=../doc/zapper' <<<"$mtree" || fail "arch: .MTREE symlink: $mtree"; ok
  grep -qxF './usr time=1791206700.0 mode=755 type=dir' <<<"$mtree" || fail "arch: .MTREE dir: $mtree"; ok
  # Two builds of one .deb are the same bytes.
  cp "$out" "$tmp/first.pkg.tar.zst"
  SOURCE_DATE_EPOCH=1791206700 "$here/package-arch.sh" "$tmp/zapper.deb" "$tmp/arch" >/dev/null
  cmp -s "$out" "$tmp/first.pkg.tar.zst" || fail "arch: not reproducible"; ok
  # Refused: a .deb without the deb marker, with an empty notice, or with an unknown dependency.
  rm "$pkg/usr/lib/zapper/install-kind"
  dpkg-deb --root-owner-group --build "$pkg" "$tmp/n.deb" >/dev/null
  refused() { ! "$here/package-arch.sh" "$tmp/n.deb" "$tmp/arch2" 2>"$tmp/err" && grep -q "$1" "$tmp/err"; }
  refused 'no usr/lib/zapper/install-kind saying deb' || fail "package-arch must refuse a .deb without install-kind: $(cat "$tmp/err")"; ok
  cp "$root/packaging/install-kind-deb" "$pkg/usr/lib/zapper/install-kind"
  : > "$pkg/usr/share/doc/zapper/LICENSE"
  dpkg-deb --root-owner-group --build "$pkg" "$tmp/n.deb" >/dev/null
  refused 'no usr/share/doc/zapper/LICENSE' || fail "package-arch must refuse an empty LICENSE: $(cat "$tmp/err")"; ok
  printf 'notice\n' > "$pkg/usr/share/doc/zapper/LICENSE"
  sed -i 's/^Depends: .*/Depends: libgtk-3-0, libfoo9/' "$pkg/DEBIAN/control"
  dpkg-deb --root-owner-group --build "$pkg" "$tmp/n.deb" >/dev/null
  refused 'unknown Debian dependencies.*libfoo9' || fail "package-arch must refuse an unknown dependency: $(cat "$tmp/err")"; ok
  # A library the .deb carries leaves its dependency out of the Arch package.
  sed -i 's/^Depends: .*/Depends: libmpv2 | libmpv1, libgtk-3-0/' "$pkg/DEBIAN/control"
  printf 'lib\n' > "$pkg/usr/lib/zapper/libmpv.so.2"
  dpkg-deb --root-owner-group --build "$pkg" "$tmp/b.deb" >/dev/null
  "$here/package-arch.sh" "$tmp/b.deb" "$tmp/arch2" >/dev/null 2>&1 || fail "package-arch on a .deb that bundles libmpv"
  b=("$tmp"/arch2/zapper-*.pkg.tar.zst)
  [ "$(zstd -dcq "${b[0]}" | tar -xO .PKGINFO | sed -n 's/^depend = //p')" = gtk3 ] || fail "arch: the bundled libmpv is still a dependency"; ok
  rm "$pkg/usr/lib/zapper/libmpv.so.2"
fi

# --- notices.py: the .deb's copyright file from LICENSE, and the Rust notices from a small
# cargo-about report with an LGPL crate (which gets its source address).
python3 "$here/notices.py" copyright "$root/LICENSE" "$tmp/copyright"
grep -qx 'Copyright: 2026 Francis Lavoie' "$tmp/copyright" || fail "copyright: no Copyright line"; ok
grep -qx 'License: MIT' "$tmp/copyright" || fail "copyright: no License: MIT"; ok
cat > "$tmp/about.json" <<'EOF'
{"licenses": [
  {"id": "MIT", "text": "MIT License\n\nCopyright (c) 2020 A\n\nPermission is hereby granted.", "source_path": "LICENSE-MIT",
   "used_by": [{"crate": {"name": "a", "version": "1.0.0", "authors": ["A <a@a>"]}}]},
  {"id": "LGPL-2.1", "text": "GNU LESSER GENERAL PUBLIC LICENSE\nCopyright (C) 1991, 1999 Free Software Foundation, Inc.\nterms", "source_path": "LICENSE",
   "used_by": [{"crate": {"name": "libmpv2", "version": "7.0.0", "authors": ["kohsine"], "repository": "https://github.com/kohsine/libmpv-rs"}}]}
]}
EOF
ZAPPER_REPO=me/zapper python3 "$here/notices.py" rust "$tmp/about.json" "$tmp/rust.txt"
grep -q '^  Copyright (c) 2020 A$' "$tmp/rust.txt" || fail "rust notices: the crate's copyright line"; ok
grep -q 'Free Software Foundation' "$tmp/rust.txt" && ! grep -q '^  Copyright (C) 1991' "$tmp/rust.txt" ||
  fail "rust notices: the LGPL's own copyright stays in its text, not under the crate"; ok
grep -q '^  crate: https://crates.io/crates/libmpv2/7.0.0$' "$tmp/rust.txt" || fail "rust notices: the LGPL crate's source"; ok
grep -q 'https://github.com/me/zapper' "$tmp/rust.txt" || fail "rust notices: zapper's own source"; ok

# --- libmpv-manifest.py: a libmpv release's SHA256SUMS into the manifest, and back out per
# platform; the committed manifest is well formed.
a=$(printf 'a%.0s' $(seq 64)) b=$(printf 'b%.0s' $(seq 64))
printf '%s  libmpv-linux-x86_64.tar.xz\n%s  libmpv-windows-x86_64.zip\n%s  mpv-0.41.0.tar.gz\n' "$a" "$b" "$a" > "$tmp/SHA256SUMS"
python3 "$here/libmpv-manifest.py" write libmpv-0.41.0-ffmpeg-8.1.3-r2 me/zapper "$tmp/SHA256SUMS" "$tmp/manifest.json"
[ "$(python3 "$here/libmpv-manifest.py" get linux-x86_64 "$tmp/manifest.json")" = \
  "https://github.com/me/zapper/releases/download/libmpv-0.41.0-ffmpeg-8.1.3-r2/libmpv-linux-x86_64.tar.xz $a" ] ||
  fail "libmpv-manifest: the Linux archive's address and sum"; ok
[ "$(jq -r '.mpv + " " + .ffmpeg' "$tmp/manifest.json")" = "0.41.0 8.1.3" ] || fail "libmpv-manifest: versions from the tag"; ok
! python3 "$here/libmpv-manifest.py" get macos-arm64 "$tmp/manifest.json" 2>/dev/null || fail "libmpv-manifest: a platform it lacks"; ok
! python3 "$here/libmpv-manifest.py" write v1.2.3 me/zapper "$tmp/SHA256SUMS" 2>/dev/null || fail "libmpv-manifest: a tag that isn't libmpv's"; ok
jq -e '(.tag | test("^libmpv-[0-9.]+-ffmpeg-[0-9.]+-r[0-9]+$")) and (.platforms | type == "object")' \
  "$root/packaging/libmpv/manifest.json" >/dev/null || fail "packaging/libmpv/manifest.json"; ok

echo "test-scripts: $n checks passed"
