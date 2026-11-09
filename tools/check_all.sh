#!/usr/bin/env bash
# Every layer, one command, no account access.
#
# The unit suites prove pieces in isolation; the harnesses prove the real ones work together:
# the recording service through a real zapper-server, and (with --host) mpv decoding and the
# player's transport on a headless host. Video comes from a local fixture
# (tools/make_fixtures.sh, served by tools/fake_stream.py), never a provider.
#
# Nothing here reads the account or the catalogue on this machine: every harness runs with
# ZAPPER_HOME pointed at a scratch directory, which has neither. (A run against the real ones
# needs ZAPPER_ALLOW_REAL_ACCOUNT=1, and this script never sets it.)
#
#   tools/check_all.sh          # tests, lint, build, the recording and service checks
#   tools/check_all.sh --host   # also the player checks, on a headless mpv host
#
# --host needs libmpv (lib/, `just fetch-libmpv`), gnome-shell and dbus-run-session:
# mpv opens its own window, so those checks run inside tools/headless_host.sh's headless
# gnome-shell, never on the display of whoever is at the machine. They play video for a few
# minutes: run them when the player, the mpv backend or the host changed, not on every edit.

set -uo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.local/bin:$PATH"
# An opt-in left exported in the calling shell is not consent for this run.
unset ZAPPER_ALLOW_REAL_ACCOUNT

HOST=0
case "${1:-}" in
  --host|--full) HOST=1 ;;
  --fast|"") ;;
  *) echo "usage: check_all.sh [--host]" >&2; exit 2 ;;
esac

PASS=0; FAIL=0
step() {
  local label="$1"; shift
  printf '\n\033[1m== %s\033[0m\n' "$label"
  if "$@"; then printf '\033[32m   ok\033[0m  %s\n' "$label"; PASS=$((PASS+1))
  else printf '\033[31m   FAIL\033[0m %s\n' "$label"; FAIL=$((FAIL+1)); fi
}

# The CI jobs' cargo commands (.github/workflows/ci.yml): zapper-core, zapper-mpv and zapper-server (pipefail keeps cargo's status).
core_tests()    { cargo test -p zapper-core --lib -q 2>&1 | tail -3; }
core_clippy()   { cargo clippy -p zapper-core --lib --features harness -q -- -D warnings 2>&1 | tail -3; }
mpv_tests()     { cargo test -p zapper-mpv --lib -q 2>&1 | tail -3; }
mpv_clippy()    { cargo clippy -p zapper-mpv --lib --bins --features harness -q -- -D warnings 2>&1 | tail -3; }
server_tests()  { CI=1 cargo test -p zapper-server --lib -q 2>&1 | tail -3; }
server_clippy() { cargo clippy -p zapper-server --lib --bins --features harness -q -- -D warnings 2>&1 | tail -3; }
# The Node in .nvmrc, through nvm when it is installed; otherwise whatever is on PATH.
use_node()    { [[ -s "$HOME/.nvm/nvm.sh" ]] || return 0; . "$HOME/.nvm/nvm.sh" && nvm use >/dev/null 2>&1; }
front_build() { use_node && npm run build 2>&1 | tail -2; }
front_checks(){
  use_node
  # Globbed rather than handed the directory: `node --test <dir>` needs a
  # recent Node, and on an older one it is read as a module path and fails
  # with MODULE_NOT_FOUND - which looks like a broken suite, not a wrong Node.
  local out; out=$(node --test src/__checks__/*.test.mjs 2>&1); local rc=$?
  # Report on node's exit code, not on matching its summary: the prefix is a
  # unicode marker that has changed between Node versions, and grepping for it
  # turned a passing suite into a failure.
  echo "$out" | grep -E '(pass|fail) [0-9]+' | head -2
  return $rc
}

TMP=$(mktemp -d)
# Config, data and cache for every harness: empty, and gone with $TMP when this script exits. See
# crates/zapper-core/src/paths.rs.
export ZAPPER_HOME="$TMP/home"
mkdir -p "$ZAPPER_HOME"
PIDS=()
stop_fixtures() { local p; for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done; PIDS=(); }
trap 'stop_fixtures; rm -rf "$TMP"' EXIT

# --- the recording service, through a real zapper-server ----------------------

server_bins() {
  cargo build -q -p zapper-server --features harness \
    --bin zapper-server --bin recording_check --bin offair_check --bin service_check 2>&1 | tail -3
}
# A harness bin run from the crate's directory, with its PASS/FAIL lines shown and its exit status
# kept. No core dumps: a crash in a test process must not write one.
harness() { # harness <bin> [args]
  local bin="$1"; shift
  (cd crates/zapper-server && ulimit -c 1 && ../../target/debug/"$bin" "$@" 2>&1 | grep -E '^(PASS|FAIL|  FAIL)'
   exit "${PIPESTATUS[0]}")
}

# --- the player checks, on a headless mpv host ---------------------------------

start_fixtures() {
  python3 tools/fake_stream.py --port 8788 --movies 2 --series 1 >"$TMP/fixtures.log" 2>&1 &
  PIDS+=($!)
  python3 tools/fake_opensubtitles.py --port 19641 >"$TMP/subs.log" 2>&1 &
  PIDS+=($!)
  sleep 2
  # Probe the index, not a fixture: the fixture endpoint is an endless chunked
  # stream with no Content-Length, so curl never sees a clean finish and
  # reports failure against a server that is working perfectly.
  curl -sf --max-time 5 http://127.0.0.1:8788/ | grep -q '\.ts'
}

# A bin run inside the headless host (tools/headless_host.sh), its PASS/FAIL line shown. The
# harnesses bound their own runs; there is no `timeout` here: uutils' implementation aborts on
# its kill path, which throws a desktop crash dialog.
on_host() { # on_host <bin> [flags]
  local bin="$1"; shift
  local log; log=$(mktemp)
  (cd crates/zapper-mpv && ../../tools/headless_host.sh ../../target/debug/"$bin" "$@" >"$log" 2>&1)
  grep -E '^(PASS|FAIL|  FAIL)' "$log" | tail -2
  # Judged on the verdict line, not the exit status: a crash in mpv's teardown after PASS (exit
  # 139, eglMakeCurrent) is a known host issue, not a failure of the check.
  grep -q '^PASS' "$log"
}

MOVIE=movie/u/p/2.mp4
EPISODE=series/u/p/100111.mkv
# pc [VAR=value ...] -- <player_check flags>: player_check against the fixture server, with the
# fixture a flag needs set for that run alone.
pc() {
  local envs=()
  while [[ "$1" != -- ]]; do envs+=("$1"); shift; done
  shift
  (
    export ZAPPER_FIXTURE_BASE=http://127.0.0.1:8788 ZAPPER_FIXTURE_NAME=sample1080.ts ZAPPER_STREAM_ID=1
    export ZAPPER_STREAM_URL=http://127.0.0.1:8788/sample1080.ts
    for e in "${envs[@]}"; do export "$e"; done
    on_host player_check "$@"
  )
}
# Catch-up, with the server and this computer in different zones.
catchup() {
  python3 tools/fake_stream.py --port 8789 --panel-utc-offset 1 --archive-days 3 >"$TMP/catchup.log" 2>&1 &
  local cs=$!
  sleep 2
  ( export ZAPPER_PANEL_BASE=http://127.0.0.1:8789 TZ=America/New_York; on_host catchup_check )
  local rc=$?
  kill "$cs" 2>/dev/null
  return $rc
}
mpv_bins() { cargo build -q -p zapper-mpv --features harness --bins 2>&1 | tail -3; }

step "core unit tests"       core_tests
step "core clippy"           core_clippy
step "mpv unit tests"        mpv_tests
step "mpv clippy"            mpv_clippy
step "frontend build"        front_build
step "server unit tests"     server_tests
step "server clippy"         server_clippy
step "frontend checks"       front_checks
step "release scripts"       scripts/test-scripts.sh
step "build the server and its checks" server_bins
step "recording through zapper-server" harness recording_check
step "a channel that is off air"       harness offair_check
step "the service installs and goes"   harness service_check

if [[ $HOST -eq 1 ]]; then
  step "build the mpv checks"       mpv_bins
  step "fixture servers"            start_fixtures
  step "player: live transport"     pc --
  step "player: keeps playing"      pc -- --keeps-playing 20
  step "player: movie"              pc ZAPPER_FIXTURE_MOVIE=$MOVIE -- --movie
  step "player: held, then resumed" pc ZAPPER_FIXTURE_MOVIE=$MOVIE -- --held
  step "player: pause then play"    pc ZAPPER_FIXTURE_MOVIE=$MOVIE -- --pause-then-play
  step "player: previews on a title" pc ZAPPER_FIXTURE_MOVIE=$MOVIE -- --trickplay-vod
  step "player: episode"            pc ZAPPER_FIXTURE_EPISODE=$EPISODE -- --episode
  step "player: up next"            pc ZAPPER_FIXTURE_EPISODE=$EPISODE -- --up-next
  step "player: subtitles"          pc ZAPPER_FIXTURE_EPISODE=$EPISODE -- --subtitles
  step "player: find subtitles"     pc ZAPPER_FIXTURE_MOVIE=movie/u/p/1.mkv ZAPPER_OPENSUBTITLES_URL=http://127.0.0.1:19641/api/v1 -- --find-subtitles
  step "player: captions"           pc ZAPPER_FIXTURE_NAME=captions169.ts -- --captions
  step "catch-up"                   catchup
  # Windowless libmpv: no host needed, but they play, so they run with the rest that do.
  step "player: endings"            harness ending_check
  step "player: recording handover" harness handover_check
  stop_fixtures
fi

printf '\n\033[1m%d passed, %d failed\033[0m' "$PASS" "$FAIL"
[[ $HOST -eq 0 ]] && printf '  \033[2m(--host also runs the player checks on a headless mpv host)\033[0m'
printf '\n'
[[ $FAIL -eq 0 ]]
