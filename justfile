set shell := ["bash", "-cu"]
# Only `package-windows` is meant for Windows: PowerShell there, never WSL's bash.
set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

# Builds use 8 jobs unless told otherwise, not every core: a cargo build at full width makes
# everything else on a desktop stutter, video included. CI sets CARGO_BUILD_JOBS to the runner's.
export CARGO_BUILD_JOBS := env_var_or_default("CARGO_BUILD_JOBS", "8")

default: test

# Every suite that needs no window, no account and no network: Rust's, the frontend's node
# checks, and the release scripts'. tools/check_all.sh adds lint, the build and the harnesses.
test: test-rust test-ui test-scripts

test-rust:
    cd src-tauri && cargo test --lib -q

test-ui:
    node --test src/__checks__/*.test.mjs

# The release scripts on throwaway copies (scripts/test-scripts.sh).
test-scripts:
    scripts/test-scripts.sh

lint:
    cd src-tauri && cargo clippy --lib --bins -q -- -D warnings
    npx tsc --noEmit

# Everything, as tools/check_all.sh runs it (`just check --windowed` adds the GL and playback
# harnesses, which open windows and play video).
check *args:
    tools/check_all.sh {{args}}

# A development build with the Vite dev server (npm run app).
dev:
    npm run app

# Prepares a release on main: sets the version everywhere, dates CHANGELOG.md's Unreleased
# section, commits "Release <version>" and tags v<version>. It never pushes; it prints the push
# command, and the pushed tag builds a draft GitHub release. See docs/releasing.md.
release version:
    @scripts/release.sh "{{version}}"

# The Linux packages, into src-tauri/target/release/bundle/: the .deb (its Version made
# Debian's by scripts/fix-deb.sh, then checked by scripts/check-deb.sh) and the .rpm, from one
# build. Through `npx tauri`, the project's own Tauri 2 CLI: a `cargo tauri` on PATH may be
# another major version.
#
# ZAPPER_RELEASE_VERSION=<version> (the release workflow) must be a release version equal to
# tauri.conf.json's, and is checked against the package's Version; unset, the package is
# whatever tauri.conf.json says.
package:
    #!/usr/bin/env bash
    set -euo pipefail
    v=$(jq -er .version src-tauri/tauri.conf.json)
    if [ -n "${ZAPPER_RELEASE_VERSION:-}" ]; then
      python3 scripts/version.py check "$ZAPPER_RELEASE_VERSION"
      [ "$ZAPPER_RELEASE_VERSION" = "$v" ] || { echo "package: ZAPPER_RELEASE_VERSION=$ZAPPER_RELEASE_VERSION doesn't match tauri.conf.json's $v" >&2; exit 1; }
    fi
    # Old packages first, so the globs below match only this build's.
    rm -f src-tauri/target/release/bundle/deb/zapper_*.deb src-tauri/target/release/bundle/rpm/zapper-*.rpm
    npx tauri build --bundles deb,rpm
    deb=(src-tauri/target/release/bundle/deb/zapper_*.deb)
    [ ${#deb[@]} = 1 ] || { echo "package: expected one .deb, found: ${deb[*]}" >&2; exit 1; }
    scripts/fix-deb.sh "${deb[0]}"
    scripts/check-deb.sh "${deb[0]}" "$v"
    ls -l src-tauri/target/release/bundle/deb/zapper_*.deb src-tauri/target/release/bundle/rpm/zapper-*.rpm

# On Windows (builds; not yet tested on screen or released): the NSIS installer and the MSI,
# with the install-kind markers the updater reads (packaging/windows/).
package-windows:
    npx tauri build --config packaging/windows/installers.conf.json

# On macOS (NOT SUPPORTED YET): zapper.app and the .dmg, with the install-kind marker in
# Contents/Resources (packaging/macos/dmg.conf.json).
package-macos:
    npx tauri build --config packaging/macos/dmg.conf.json
