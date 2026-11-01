#!/usr/bin/env bash
# Usage: scripts/test-scripts.sh  (`just test-scripts`)
#
# The release scripts, on throwaway copies: the version rules and their order, the changelog's
# release and notes, set-version on copies of the real files (only the versions change), the
# placeholder repository refused by release.sh, and fix-deb/check-deb on a small fake .deb.
# Nothing here touches the repository's own files, its tags or the network.
set -euo pipefail
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
if command -v dpkg-deb >/dev/null; then
  pkg=$tmp/pkg
  mkdir -p "$pkg/DEBIAN" "$pkg/usr/bin" "$pkg/usr/lib/zapper" "$pkg/usr/share/applications"
  printf '#!/bin/sh\n' > "$pkg/usr/bin/zapper"; chmod 755 "$pkg/usr/bin/zapper"
  cp "$root/packaging/install-kind-deb" "$pkg/usr/lib/zapper/install-kind"
  printf '[Desktop Entry]\nType=Application\nName=zapper\nExec=zapper\nIcon=zapper\nCategories=AudioVideo;Video;\nStartupWMClass=zapper\n' > "$pkg/usr/share/applications/zapper.desktop"
  printf 'Package: zapper\nVersion: 0.2.0-rc.1\nArchitecture: amd64\nMaintainer: t\nDepends: libmpv2 | libmpv1, libwebkit2gtk-4.1-0, libgtk-3-0\nDescription: t\n' > "$pkg/DEBIAN/control"
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
fi

echo "test-scripts: $n checks passed"
