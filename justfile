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

# The libmpv zapper links and ships (docs/licensing.md): the LGPL build of libmpv and FFmpeg
# that packaging/libmpv/manifest.json names for this platform, downloaded, checked against its
# SHA-256 and unpacked into src-tauri/lib/<platform>/. `just fetch-libmpv --archive <file>`
# takes one built by `just build-libmpv` instead. On Windows: tools/fetch-libmpv.ps1.
fetch-libmpv *args:
    scripts/fetch-libmpv.sh {{args}}

# Builds that libmpv here, from the pinned sources in packaging/libmpv/sources.txt, into
# target-libmpv/ (normally the libmpv workflow does, .github/workflows/libmpv.yml). Four jobs
# unless LIBMPV_JOBS says otherwise; expect most of an hour.
build-libmpv platform="linux-x86_64":
    scripts/build-libmpv.sh {{platform}} target-libmpv/work target-libmpv/out

# Prepares a release on main: sets the version everywhere, dates CHANGELOG.md's Unreleased
# section, commits "Release <version>" and tags v<version>. It never pushes; it prints the push
# command, and the pushed tag builds a draft GitHub release. See docs/releasing.md.
release version:
    @scripts/release.sh "{{version}}"

# The Linux packages, into src-tauri/target/release/bundle/: the .deb (its Version made
# Debian's by scripts/fix-deb.sh, then checked by scripts/check-deb.sh) and the .rpm, from one
# build, then the Arch package made from the .deb (`just package-arch`). Through `npx tauri`, the project's own Tauri 2 CLI: a `cargo tauri` on PATH may be
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
    # The bundled libmpv and FFmpeg the packages carry in /usr/lib/zapper (docs/licensing.md),
    # then zapper-server, the recording service they install as /usr/bin/zapper-server, built
    # against that FFmpeg (src-tauri/build.rs).
    scripts/fetch-libmpv.sh linux-x86_64
    cargo build --release --manifest-path src-tauri/Cargo.toml --no-default-features --features server --bin zapper-server
    # Old packages first, so the globs below match only this build's.
    rm -f src-tauri/target/release/bundle/deb/zapper_*.deb src-tauri/target/release/bundle/rpm/zapper-*.rpm
    # The license notices the packages ship (docs/licensing.md): the Rust crates' and the .deb's
    # copyright file into src-tauri/target/licenses, unless Cargo.lock, the manifest, about.toml
    # and LICENSE are as they were for the last ones; the interface's come from the UI build
    # `tauri build` runs. Either fails the build on a license outside about.toml's allow-list.
    scripts/licenses.sh --if-changed
    npx tauri build --bundles deb,rpm
    deb=(src-tauri/target/release/bundle/deb/zapper_*.deb)
    [ ${#deb[@]} = 1 ] || { echo "package: expected one .deb, found: ${deb[*]}" >&2; exit 1; }
    scripts/fix-deb.sh "${deb[0]}"
    scripts/check-deb.sh "${deb[0]}" "$v"
    scripts/package-arch.sh "${deb[0]}" src-tauri/target/release/bundle/arch
    ls -l src-tauri/target/release/bundle/deb/zapper_*.deb src-tauri/target/release/bundle/rpm/zapper-*.rpm \
      src-tauri/target/release/bundle/arch/zapper-*-x86_64.pkg.tar.zst

# The Arch package (src-tauri/target/release/bundle/arch/zapper-<pkgver>-1-x86_64.pkg.tar.zst),
# made from the .deb of `just package` without makepkg: the same payload with the install-kind
# marker saying arch, pacman's .PKGINFO and .MTREE, and Depends mapped through the table in
# scripts/arch-pkg.py. `just package` already runs it; this redoes it from the last .deb.
package-arch:
    scripts/package-arch.sh src-tauri/target/release/bundle/deb/zapper_*.deb src-tauri/target/release/bundle/arch

# Installs that package in a throwaway archlinux:latest container and checks it (needs Docker and
# the network, so `just package` doesn't run it): dependencies, pacman -Qkk, the marker, the
# license notices, ldd and a headless launch. See scripts/check-arch-pkg.sh.
check-arch-pkg:
    scripts/check-arch-pkg.sh src-tauri/target/release/bundle/arch/zapper-*-x86_64.pkg.tar.zst

# On Windows (builds; not yet tested on screen or released): the NSIS installer and the MSI,
# with the install-kind markers the updater reads and the license notices in licenses\ beside
# zapper.exe (packaging/windows/). The notices come from scripts/licenses.sh run by Git for
# Windows' bash, with Windows' crate graph; it needs cargo-about and Python.
package-windows:
    & ./tools/fetch-libmpv.ps1; if ($LASTEXITCODE) { exit $LASTEXITCODE }
    cargo build --release --manifest-path src-tauri/Cargo.toml --no-default-features --features server --bin zapper-server; if ($LASTEXITCODE) { exit $LASTEXITCODE }
    $env:PYTHON = 'python'; & "$env:ProgramFiles\Git\bin\bash.exe" scripts/licenses.sh --if-changed; if ($LASTEXITCODE) { exit $LASTEXITCODE }
    npx tauri build --config packaging/windows/installers.conf.json

# On macOS (NOT SUPPORTED YET): zapper.app and the .dmg, with the install-kind marker and the
# license notices in Contents/Resources (packaging/macos/dmg.conf.json).
package-macos:
    scripts/fetch-libmpv.sh
    scripts/licenses.sh --if-changed
    npx tauri build --config packaging/macos/dmg.conf.json

# The third-party license notices the packages ship (docs/licensing.md): the Rust crates' into
# src-tauri/target/licenses (scripts/licenses.sh, needs cargo-about), then the UI build, which
# writes dist/licenses/ with the interface's and copies the rest in beside it for Settings >
# About. Fails on a license outside the allow-list in about.toml. `just package` runs the same.
licenses:
    scripts/licenses.sh
    npm run build
