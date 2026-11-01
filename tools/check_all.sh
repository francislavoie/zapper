#!/usr/bin/env bash
# Every layer, one command, no account access.
#
# The unit suites prove pieces in isolation; the harnesses prove the real
# ones work together — GL compositing, mpv decode and transport, and the
# catalogue commands over a large fixture catalogue. Video comes from a local
# fixture (tools/make_fixtures.sh, served by tools/fake_stream.py), never a
# provider.
#
# Nothing here reads the account or the catalogue on this machine: every
# harness and the app itself run with ZAPPER_HOME pointed at a scratch
# directory, which has neither. (A run against the real ones needs
# ZAPPER_ALLOW_REAL_ACCOUNT=1, and this script never sets it.)
#
#   tools/check_all.sh            # tests, lint, build, the fixture catalogue
#   tools/check_all.sh --windowed # also the GL and playback harnesses
#
# The windowed harnesses open real windows and play video, which interrupts
# whoever is at the machine. They are opt-in for that reason: run them when
# surface, mpv or rect behaviour changed, not on every edit.

set -uo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.local/bin:$PATH"
# An opt-in left exported in the calling shell is not consent for this run.
unset ZAPPER_ALLOW_REAL_ACCOUNT

WINDOWED=0
case "${1:-}" in
  --windowed|--full) WINDOWED=1 ;;
  --fast|"") ;;
  *) echo "usage: check_all.sh [--windowed]" >&2; exit 2 ;;
esac

PASS=0; FAIL=0
step() {
  local label="$1"; shift
  printf '\n\033[1m== %s\033[0m\n' "$label"
  if "$@"; then printf '\033[32m   ok\033[0m  %s\n' "$label"; PASS=$((PASS+1))
  else printf '\033[31m   FAIL\033[0m %s\n' "$label"; FAIL=$((FAIL+1)); fi
}

rust_tests()  { (cd src-tauri && cargo test --lib -q 2>&1 | tail -3); }
rust_clippy() { (cd src-tauri && cargo clippy --lib --bins -q -- -D warnings 2>&1 | tail -3); }
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
ipc_check()   { (cd src-tauri && cargo run -q --features harness --bin ipc_check 2>&1 | grep -E '^(PASS|FAIL|  FAIL)'); }
# A provider ending and refusing streams, imitated by fake_stream.py (which it
# starts itself), each one explained and none reopened behind the viewer.
ending_check(){
  (cd src-tauri && ./target/debug/ending_check 2>&1 | grep -E '^(PASS|FAIL|  FAIL)'
   exit "${PIPESTATUS[0]}")
}

# --- windowed harnesses -----------------------------------------------------

SRV=""
start_fixtures() {
  python3 tools/fake_stream.py --port 8788 >"$TMP/fixtures.log" 2>&1 &
  SRV=$!
  sleep 2
  # Probe the index, not a fixture: the fixture endpoint is an endless chunked
  # stream with no Content-Length, so curl never sees a clean finish and
  # reports failure against a server that is working perfectly.
  curl -sf --max-time 5 http://127.0.0.1:8788/ | grep -q '\.ts' 
}
stop_fixtures() { [[ -n "$SRV" ]] && kill "$SRV" 2>/dev/null; SRV=""; }
TMP=$(mktemp -d)
# Config, data and cache for every harness and the app: empty, and gone with
# $TMP when this script exits. See src-tauri/src/paths.rs.
export ZAPPER_HOME="$TMP/home"
mkdir -p "$ZAPPER_HOME"
# The session's Wayland socket, with the usual defaults for a shell that wasn't
# started inside the session (an SSH login, say).
WL=${WAYLAND_DISPLAY:-wayland-0}
XRD=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
trap 'stop_fixtures; rm -rf "$TMP"' EXIT

# Not `timeout`: uutils' implementation aborts on its kill path, which throws
# a desktop crash dialog. Poll, then kill by PID.
run_windowed() {
  local bin="$1" secs="$2" log; log=$(mktemp)
  ( cd src-tauri && WAYLAND_DISPLAY=$WL XDG_RUNTIME_DIR=$XRD \
      GDK_BACKEND=wayland ZAPPER_HARNESS=1 ZAPPER_FIXTURE_BASE=http://127.0.0.1:8788 \
      ZAPPER_FIXTURE_NAME=sample1080.ts ZAPPER_STREAM_ID=1 \
      ZAPPER_STREAM_URL=http://127.0.0.1:8788/sample1080.ts \
      ./target/debug/"$bin" >"$log" 2>&1 & echo $! >"$TMP/win.pid" )
  local pid; pid=$(cat "$TMP/win.pid")
  for _ in $(seq 1 "$secs"); do
    sleep 1
    grep -qE '^(PASS|FAIL)' "$log" && break
    kill -0 "$pid" 2>/dev/null || break
  done
  kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
  grep -E '^(PASS|FAIL)' "$log" | tail -1
  grep -q '^PASS' "$log"
}

# The real binary, not a harness.
#
# Every other windowed check runs a purpose-built bin that calls into the
# library directly, so none of them executes `run()` — and a panic in there
# is a panic before the app has a window, which is as broken as it gets. For
# example, `gdk::set_program_class` asserts GDK is initialised while the app
# id has to be set before Tauri creates anything: get that order wrong and
# the app dies on launch while every other check passes.
#
# Nothing is asserted about what it draws. Still alive after a few seconds,
# and no panic on the way, is the entire question.
app_starts() {
  local log; log=$(mktemp)
  ( cd src-tauri && WAYLAND_DISPLAY=$WL XDG_RUNTIME_DIR=$XRD \
      GDK_BACKEND=wayland ZAPPER_HARNESS=1 ./target/debug/zapper >"$log" 2>&1 & echo $! >"$TMP/app.pid" )
  local pid; pid=$(cat "$TMP/app.pid")
  sleep 6
  local alive=0
  kill -0 "$pid" 2>/dev/null && alive=1
  kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
  if grep -qiE 'panicked at' "$log"; then
    grep -iE 'panicked at' "$log" | head -2
    return 1
  fi
  if [[ $alive -eq 0 ]]; then
    echo "the app exited within 6s of starting"
    tail -3 "$log"
    return 1
  fi
  echo "still running after 6s, no panic"
}

step "rust unit tests"      rust_tests
step "rust clippy"          rust_clippy
step "frontend build"       front_build
step "frontend checks"      front_checks
step "release scripts"      scripts/test-scripts.sh
step "catalogue over a fixture mirror" ipc_check

if [[ $WINDOWED -eq 1 ]]; then
  (cd src-tauri && cargo build -q --bins --features harness 2>&1 | tail -2)
  step "fixture server"     start_fixtures
  step "the app starts"        app_starts
  step "surface: GL + decode"  run_windowed surface_check 40
  step "player: transport"     run_windowed player_check 70
  # No window and no sound, but it plays: here with the rest that do.
  step "player: endings"       ending_check
fi

printf '\n\033[1m%d passed, %d failed\033[0m' "$PASS" "$FAIL"
[[ $WINDOWED -eq 0 ]] && printf '  \033[2m(--windowed also runs the GL and playback harnesses)\033[0m'
printf '\n' 
[[ $FAIL -eq 0 ]]
