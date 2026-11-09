set shell := ["bash", "-cu"]
# Recipes are for Linux and macOS; on Windows the release workflow runs the same commands in PowerShell.
set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

# Builds use 8 jobs unless told otherwise, not every core: a cargo build at full width makes
# everything else on a desktop stutter, video included. CI sets CARGO_BUILD_JOBS to the runner's.
export CARGO_BUILD_JOBS := env_var_or_default("CARGO_BUILD_JOBS", "8")

default: test

# Every suite that needs no window, no account and no network: Rust's, the frontend's node
# checks, and the release scripts'. tools/check_all.sh adds lint, the build and the harnesses.
test: test-rust test-ui test-scripts

test-rust:
    cargo test -p zapper --lib -q

test-ui:
    node --test src/__checks__/*.test.mjs

# The release scripts on throwaway copies (scripts/test-scripts.sh).
test-scripts:
    scripts/test-scripts.sh

lint:
    cargo clippy -p zapper --lib --bins -q -- -D warnings
    cargo clippy -p zapper --lib --bins --features harness,mpv -q -- -D warnings
    cargo clippy -p zapper --lib --bins --no-default-features --features server -q -- -D warnings
    npx tsc --noEmit

# Everything, as tools/check_all.sh runs it (`just check --host` adds the player checks, which
# play video in mpv on a headless host).
check *args:
    tools/check_all.sh {{args}}

# The player checks alone, each flag against the fixture streams, on a headless mpv host: the
# same as `just check --host` after its unit suites.
check-host:
    tools/check_all.sh --host

# zapper-server with the browser interface compiled in (the interface is built first:
# crates/zapper/build.rs embeds dist/). Run it with target/release/zapper-server; see README.md.
server:
    npm run build
    cargo build --release -p zapper --no-default-features --features server --bin zapper-server

# The browser interface on its own, against the mock backend (http://localhost:5180).
dev:
    npm run dev

# The libmpv zapper links and ships (docs/licensing.md): the LGPL build of libmpv and FFmpeg
# that packaging/libmpv/manifest.json names for this platform, downloaded, checked against its
# SHA-256 and unpacked into crates/zapper/lib/<platform>/. `just fetch-libmpv --archive <file>`
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

# zapper-server for Linux as one static file (docs/recording.md, "Without the app"):
# target/release/bundle/server/zapper-server-<version>-linux-x86_64, built for musl
# with a minimal static LGPL FFmpeg and its licence notices built in, in a pinned Rust image on
# Alpine (needs Docker or Podman, the interface built and the Rust notices). See
# scripts/build-server-static.sh.
package-server:
    npm run build
    scripts/licenses.sh --if-changed
    scripts/build-server-static.sh target/release/bundle/server --with-checks

# Checks it: statically linked, run in bare debian:stable-slim and alpine containers and from
# scratch, its sample Dockerfile, and the fixture streams recorded through it (needs Docker or
# Podman and the network). See scripts/check-server-binary.sh.
check-server-binary:
    scripts/check-server-binary.sh target/release/bundle/server/zapper-server-*-linux-x86_64 target/release/bundle/server/checks

# The third-party license notices the server ships (docs/licensing.md): the Rust crates' into
# target/licenses (scripts/licenses.sh, needs cargo-about), then the UI build, which
# writes dist/licenses/ with the interface's and copies the rest in beside it. Fails on a
# license outside the allow-list in about.toml. `just package-server` runs the same.
licenses:
    scripts/licenses.sh
    npm run build
