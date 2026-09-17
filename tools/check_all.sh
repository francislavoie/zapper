#!/usr/bin/env bash
# Every layer, one command, no account access.
#
# The unit suites prove pieces in isolation; the harnesses prove the real
# ones work together — GL compositing, mpv decode and transport, and the
# catalogue commands over the local mirror (a synced database in
# ~/.local/share/zapper). Video comes from a local fixture
# (tools/make_fixtures.sh, served by tools/fake_stream.py), never a provider.
#
#   tools/check_all.sh            # tests, lint, build, and the local mirror
#   tools/check_all.sh --windowed # also the GL and playback harnesses
#
# The windowed harnesses open real windows and play video, which interrupts
# whoever is at the machine. They are opt-in for that reason: run them when
# surface, mpv or rect behaviour changed, not on every edit.

set -uo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.local/bin:$PATH"

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
front_build() { . "$HOME/.nvm/nvm.sh" && nvm use >/dev/null 2>&1 && npm run build 2>&1 | tail -2; }
front_checks(){
  . "$HOME/.nvm/nvm.sh" && nvm use >/dev/null 2>&1
  local out; out=$(node --test src/__checks__/ 2>&1); local rc=$?
  # Report on node's exit code, not on matching its summary: the prefix is a
  # unicode marker that has changed between Node versions, and grepping for it
  # turned a passing suite into a failure.
  echo "$out" | grep -E '(pass|fail) [0-9]+' | head -2
  return $rc
}
ipc_check()   { (cd src-tauri && cargo run -q --bin ipc_check 2>&1 | grep -E '^(PASS|FAIL|  FAIL)'); }

# --- windowed harnesses -----------------------------------------------------

SRV=""
start_fixtures() {
  python3 tools/fake_stream.py --port 8788 >/tmp/zapper-fixtures.log 2>&1 &
  SRV=$!
  sleep 2
  # Probe the index, not a fixture: the fixture endpoint is an endless chunked
  # stream with no Content-Length, so curl never sees a clean finish and
  # reports failure against a server that is working perfectly.
  curl -sf --max-time 5 http://127.0.0.1:8788/ | grep -q '\.ts' 
}
stop_fixtures() { [[ -n "$SRV" ]] && kill "$SRV" 2>/dev/null; SRV=""; }
trap stop_fixtures EXIT

# Not `timeout`: uutils' implementation aborts on its kill path, which throws
# a desktop crash dialog. Poll, then kill by PID.
run_windowed() {
  local bin="$1" secs="$2" log; log=$(mktemp)
  ( cd src-tauri && WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 \
      GDK_BACKEND=wayland ZAPPER_FIXTURE_BASE=http://127.0.0.1:8788 \
      ZAPPER_FIXTURE_NAME=sample1080.ts ZAPPER_STREAM_ID=1 \
      ZAPPER_STREAM_URL=http://127.0.0.1:8788/sample1080.ts \
      ./target/debug/"$bin" >"$log" 2>&1 & echo $! >/tmp/zapper-win.pid )
  local pid; pid=$(cat /tmp/zapper-win.pid)
  for _ in $(seq 1 "$secs"); do
    sleep 1
    grep -qE '^(PASS|FAIL)' "$log" && break
    kill -0 "$pid" 2>/dev/null || break
  done
  kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
  grep -E '^(PASS|FAIL)' "$log" | tail -1
  grep -q '^PASS' "$log"
}

step "rust unit tests"      rust_tests
step "rust clippy"          rust_clippy
step "frontend build"       front_build
step "frontend checks"      front_checks
step "catalogue over the local mirror" ipc_check

if [[ $WINDOWED -eq 1 ]]; then
  (cd src-tauri && cargo build -q --bins 2>&1 | tail -2)
  step "fixture server"     start_fixtures
  step "surface: GL + decode"  run_windowed surface_check 40
  step "player: transport"     run_windowed player_check 70
fi

printf '\n\033[1m%d passed, %d failed\033[0m' "$PASS" "$FAIL"
[[ $WINDOWED -eq 0 ]] && printf '  \033[2m(--windowed also runs the GL and playback harnesses)\033[0m'
printf '\n' 
[[ $FAIL -eq 0 ]]
