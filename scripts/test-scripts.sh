#!/usr/bin/env bash
# Usage: scripts/test-scripts.sh  (`just test-scripts`)
#
# The release scripts, on throwaway copies: the version rules and their order, the changelog's
# release and notes, set-version on copies of the real files (only the versions change), the
# placeholder repository refused by release.sh, the version the workflows read from the
# workspace, and the license helpers in notices.py.
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
[ "$(v current "$root")" = "$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml")" ] || fail "current: the workspace version"; ok
v check "$(v current "$root")" || fail "the workspace version isn't a release version"; ok
for good in 0.1.0 1.2.3 0.2.0-rc.1 0.2.0-alpha.1.2 10.0.0-beta; do
  v check "$good" || fail "$good should be a release version"; ok
done
for bad in v0.1.0 0.1 01.2.3 0.1.0+abc 0.1.0-RC.1 0.1.0-alpha10 0.1.0-1 ""; do
  ! v check "$bad" 2>/dev/null || fail "'$bad' should be refused"; ok
done
order=(0.1.0-alpha.1 0.1.0-alpha.2 0.1.0-beta 0.1.0-rc.1 0.1.0 0.1.1 0.2.0-rc.1 0.2.0 0.10.0 1.0.0)
for ((i = 1; i < ${#order[@]}; i++)); do
  [ "$(v cmp "${order[i-1]}" "${order[i]}")" = -1 ] || fail "${order[i-1]} < ${order[i]}"; ok
  # dpkg must agree with SemVer once `version.py deb` has rewritten the version.
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
# The section must exist; it is empty right after a release, and `just release` is what refuses an
# empty one.
grep -qx '## \[Unreleased\]' "$root/CHANGELOG.md" || fail "CHANGELOG.md has no Unreleased section"; ok

# --- set-version.py, on copies laid out like the repository
copy=$tmp/repo
mkdir -p "$copy/scripts"
cp "$here/set-version.py" "$copy/scripts/"
cp "$root/package.json" "$root/package-lock.json" "$copy/"
cp "$root/Cargo.toml" "$root/Cargo.lock" "$copy/"
python3 "$copy/scripts/set-version.py" 9.8.7-rc.1 >/dev/null
[ "$(v current "$copy")" = 9.8.7-rc.1 ] || fail "version.py current reads the workspace version"; ok
[ "$(jq -r .version "$copy/package.json")" = 9.8.7-rc.1 ] || fail "package.json"; ok
[ "$(jq -r '.version + " " + .packages[""].version' "$copy/package-lock.json")" = "9.8.7-rc.1 9.8.7-rc.1" ] || fail "package-lock.json"; ok
grep -q '^version = "9.8.7-rc.1"$' "$copy/Cargo.toml" || fail "Cargo.toml"; ok
for crate in zapper-core zapper-mpv zapper-server; do
  [ "$(grep -A1 "^name = \"$crate\"\$" "$copy/Cargo.lock" | tail -1)" = 'version = "9.8.7-rc.1"' ] || fail "Cargo.lock: $crate"; ok
done
for f in package.json package-lock.json Cargo.toml Cargo.lock; do
  changed=$(diff "$root/$f" "$copy/$f" | grep -c '^>' || true)
  most=2; [ "$f" = Cargo.lock ] && most=3
  [ "$changed" -ge 1 ] && [ "$changed" -le "$most" ] || fail "$f: $changed lines changed, expected only the version"; ok
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
