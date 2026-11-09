#!/usr/bin/env bash
# Usage: scripts/release.sh <version>  (`just release <version>`, e.g. `just release 0.2.0`)
#
# Prepares a release on main, locally: sets the version (scripts/set-version.py), turns
# CHANGELOG.md's Unreleased section into "## [<version>] - <today>", commits "Release <version>"
# and creates the annotated tag v<version>. It never pushes; it prints the push command, and the
# pushed tag starts .github/workflows/release.yml (docs/releasing.md).
#
# Refuses, before changing anything: an invalid version (scripts/version.py), a branch other than
# main, uncommitted changes to tracked files, main behind origin/main, an existing tag, a version
# not newer than every v* tag, or the updater's releases repository still being the placeholder
# (a release whose installs cannot find the next one would strand everyone who installs it). If a
# later step fails, the edited files are restored.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
fail() { echo "release: $*" >&2; exit 1; }
v=${1:?usage: release.sh <version>}
cd "$(git -C "$here" rev-parse --show-toplevel)"

python3 scripts/version.py check "$v" || exit 1
repo=$(scripts/releases-repo.sh) || exit 1
case "$repo" in
  OWNER/*) fail "RELEASES_REPO in crates/zapper/src/updates/mod.rs is still the placeholder '$repo'; set it first (docs/releasing.md, One-time setup)" ;;
esac
branch=$(git symbolic-ref -q --short HEAD) || branch="(detached HEAD)"
[ "$branch" = main ] || fail "not on main (on $branch); releases are cut from main"
[ -z "$(git status --porcelain --untracked-files=no)" ] || fail "uncommitted changes; commit them first"
if git rev-parse -q --verify refs/remotes/origin/main >/dev/null; then
  git merge-base --is-ancestor origin/main HEAD || fail "main is behind origin/main; pull first"
fi
! git rev-parse -q --verify "refs/tags/v$v" >/dev/null || fail "the tag v$v already exists"
while read -r tag; do
  python3 scripts/version.py check "${tag#v}" 2>/dev/null || continue
  [ "$(python3 scripts/version.py cmp "$v" "${tag#v}")" = 1 ] || fail "$v is not newer than $tag"
done < <(git tag -l 'v*')

files=(Cargo.toml Cargo.lock package.json package-lock.json CHANGELOG.md)
committed=
trap '[ -n "$committed" ] || git checkout -q -- "${files[@]}"' EXIT
python3 scripts/set-version.py "$v"
python3 scripts/changelog.py release "$v" "$(date +%F)" CHANGELOG.md
git add -- "${files[@]}"
git commit -q -m "Release $v"
committed=1
git tag -a "v$v" -m "zapper $v"
echo
echo "Committed \"Release $v\" and tagged v$v. Nothing is pushed yet. Push both to start the release build:"
echo "  git push origin main v$v"
echo "Then review the draft release on GitHub (docs/releasing.md)."
