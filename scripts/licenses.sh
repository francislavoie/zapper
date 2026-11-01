#!/usr/bin/env bash
# Usage: scripts/licenses.sh [--if-changed] [out-dir]   (default: src-tauri/target/licenses)
#
# The license files the packages ship that don't come from the UI build (the UI build writes
# THIRD-PARTY-NOTICES-ui.txt itself, see tools/licenses.mjs, and copies these in beside it for
# Settings > About > Open source licenses):
#   THIRD-PARTY-NOTICES-rust.txt  every crate zapper links, grouped by license (cargo-about, then
#                                 scripts/notices.py); fails on a license about.toml doesn't
#                                 accept, so a copyleft or unknown license stops the build
#   copyright                     the .deb's /usr/share/doc/zapper/copyright (DEP-5)
# zapper's LICENSE and packaging/licenses/ (libmpv's notice, the GPL) are committed files, which
# the packages and the UI build take from where they are. docs/licensing.md describes the flow.
#
# The crate graph is the one of the platform it runs on: Linux's (about.toml's target), Windows'
# in Git for Windows' bash (`just package-windows`, the release workflow; Python as $PYTHON), or
# this Mac's.
#
# --if-changed (`just package`): does nothing when the files in out-dir were made from the same
# inputs (Cargo.lock, Cargo.toml, about.toml, LICENSE, these scripts, the target) and haven't
# changed since; the key is in <out-dir>.key. A change to any of them reruns cargo-about, so a
# dependency under a license about.toml refuses still fails the build. ZAPPER_REBUILD=1, and every
# run without the flag (`just licenses`), regenerates.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
if_changed=
[ "${1:-}" = --if-changed ] && { if_changed=1; shift; }
[ "${ZAPPER_REBUILD:-}" = 1 ] && if_changed=
out=${1:-$root/src-tauri/target/licenses}
outputs=(THIRD-PARTY-NOTICES-rust.txt copyright)
fail() { echo "licenses: $*" >&2; exit 1; }
command -v cargo-about >/dev/null ||
  fail "cargo-about isn't installed: cargo install cargo-about --version 0.9.2 --locked --features cli"
mkdir -p "$out"
python=${PYTHON:-python3}
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) target=(--target x86_64-pc-windows-msvc) ;;
  Darwin)
    case "$(uname -m)" in
      arm64) target=(--target aarch64-apple-darwin) ;;
      *) target=(--target x86_64-apple-darwin) ;;
    esac ;;
  *) target=() ;;
esac
ZAPPER_REPO=$("$here/releases-repo.sh")
export ZAPPER_REPO

# --if-changed: what the files are made from, and what they were when made from it.
key_file=$out.key
inputs_key() {
  {
    echo "target=${target[*]-} repo=$ZAPPER_REPO $(cargo about --version)"
    (cd "$root" && sha256sum src-tauri/Cargo.lock src-tauri/Cargo.toml about.toml LICENSE \
      scripts/licenses.sh scripts/notices.py)
  } | sha256sum | cut -d' ' -f1
}
# A missing output is part of the key (and makes it differ), not a reason to stop here.
outputs_key() { (cd "$out" && sha256sum "${outputs[@]}" 2>&1 || true) | sha256sum | cut -d' ' -f1; }
if [ -n "$if_changed" ]; then
  key="$(inputs_key) $(outputs_key)"
  if [ -f "$key_file" ] && [ "$(cat "$key_file")" = "$key" ]; then
    echo "licenses: $out is up to date (same Cargo.lock, manifests and about.toml; ZAPPER_REBUILD=1 regenerates)"
    exit 0
  fi
fi
rm -f "$key_file"

# --locked: the notices describe exactly what Cargo.lock builds.
cargo about generate --format json --locked --fail -c "$root/about.toml" ${target[@]+"${target[@]}"} \
  -m "$root/src-tauri/Cargo.toml" -o "$out/about.json"
"$python" "$here/notices.py" rust "$out/about.json" "$out/THIRD-PARTY-NOTICES-rust.txt"
rm "$out/about.json"
"$python" "$here/notices.py" copyright "$root/LICENSE" "$out/copyright"

for f in "${outputs[@]}"; do
  [ -s "$out/$f" ] || fail "$out/$f is missing or empty"
done
[ -z "$if_changed" ] || echo "$(inputs_key) $(outputs_key)" > "$key_file"
echo "licenses: wrote ${outputs[*]} to $out"
echo "licenses: Rust crates by license:"
sed -n '/^Summary$/,/^$/p' "$out/THIRD-PARTY-NOTICES-rust.txt" | tail -n +3
