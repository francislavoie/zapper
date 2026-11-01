#!/usr/bin/env bash
# Usage: scripts/releases-repo.sh
#
# Prints the `owner/repo` the updater takes its releases from: RELEASES_REPO in
# src-tauri/src/updates/mod.rs, the one place it is set. Read here so `just release` and the
# release workflow can refuse a build whose updater points at the placeholder, or at a
# repository other than the one publishing it.
set -euo pipefail
src="$(cd "$(dirname "$0")/.." && pwd)/src-tauri/src/updates/mod.rs"
repo=$(sed -n 's/^pub const RELEASES_REPO: &str = "\([^"]*\)";.*$/\1/p' "$src")
[ -n "$repo" ] || { echo "releases-repo: no RELEASES_REPO in $src" >&2; exit 1; }
[ "$(printf '%s\n' "$repo" | wc -l)" = 1 ] || { echo "releases-repo: more than one RELEASES_REPO in $src" >&2; exit 1; }
echo "$repo"
