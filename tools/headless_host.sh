#!/usr/bin/env bash
# Runs a command inside a headless GNOME session, so anything that opens a window (mpv's own,
# with `vo=gpu-next`) has a Wayland compositor to open it on and never touches the display of
# whoever is at the machine.
#
#   tools/headless_host.sh target/debug/player_check --held
#
# The session is a gnome-shell with a virtual 1920x1080 monitor, under its own D-Bus session, with
# a scratch HOME and XDG_RUNTIME_DIR (short, under /tmp) exported before the session starts, so
# no session service looks in /run/user/<uid>. The command gets WAYLAND_DISPLAY at the shell's
# socket, no DISPLAY, ZAPPER_HARNESS=1, ZAPPER_MUTE=1 (no sound), and no core dumps. Its exit
# status is the command's. Needs gnome-shell and dbus-run-session.
set -uo pipefail
[ $# -ge 1 ] || { echo "usage: headless_host.sh <command> [args...]" >&2; exit 2; }
command -v gnome-shell >/dev/null && command -v dbus-run-session >/dev/null ||
  { echo "headless_host: gnome-shell and dbus-run-session are needed" >&2; exit 2; }

base=$(mktemp -d /tmp/zh.XXXXXX)
mkdir -m 700 "$base/run" "$base/home"
export HOME="$base/home" XDG_RUNTIME_DIR="$base/run"
export ZAPPER_HARNESS=1 ZAPPER_MUTE=1
unset DISPLAY WAYLAND_DISPLAY
trap 'rm -rf "$base"' EXIT

dbus-run-session -- bash -c '
  ulimit -c 1
  gnome-shell --headless --wayland --virtual-monitor 1920x1080 >"$HOME/shell.log" 2>&1 &
  shell=$!
  for _ in $(seq 1 100); do
    [ -S "$XDG_RUNTIME_DIR/wayland-0" ] && break
    kill -0 "$shell" 2>/dev/null || { echo "headless_host: gnome-shell exited"; tail -5 "$HOME/shell.log"; exit 1; }
    sleep 0.2
  done
  [ -S "$XDG_RUNTIME_DIR/wayland-0" ] || { echo "headless_host: no Wayland socket"; kill "$shell"; exit 1; }
  export WAYLAND_DISPLAY=wayland-0
  "$@"
  rc=$?
  kill "$shell" 2>/dev/null; wait "$shell" 2>/dev/null
  exit $rc
' headless_host "$@"
