#!/usr/bin/env bash
# Generates the synthetic test streams tools/fake_stream.py serves: ffmpeg's
# test pattern and a tone, encoded the way live IPTV feeds usually are
# (H.264 8-bit 4:2:0 in MPEG-TS, a keyframe every two seconds).
#
#   tools/make_fixtures.sh              # the three fixtures below, 30 s each
#   tools/make_fixtures.sh --force      # regenerate ones that already exist
#
# They go to $ZAPPER_FIXTURES, by default ~/.cache/zapper/fixtures, outside
# the tree: they are build output, and *.ts is ignored here anyway.
#
#   sample1080.ts   h264 1920x1080@50, aac   the pattern with little motion
#   motion1080p.ts  h264 1920x1080@25, aac   the whole frame scrolling
#   sport720p60.ts  h264 1280x720@60,  ac3   60 fps, high motion, AC-3 audio
set -euo pipefail

dir=${ZAPPER_FIXTURES:-$HOME/.cache/zapper/fixtures}
seconds=30
force=0
for arg in "$@"; do
  case "$arg" in
    --force) force=1 ;;
    *) echo "usage: make_fixtures.sh [--force]" >&2; exit 2 ;;
  esac
done
command -v ffmpeg >/dev/null || { echo "make_fixtures: ffmpeg isn't installed" >&2; exit 1; }
mkdir -p "$dir"

# make <name> <size> <fps> <extra video filter or ""> <audio codec>
make() {
  local out="$dir/$1" size=$2 fps=$3 vf=$4 acodec=$5
  if [[ -e "$out" && $force -eq 0 ]]; then
    echo "exists: $out (--force regenerates)"
    return
  fi
  local filter="testsrc2=size=$size:rate=$fps"
  [[ -n "$vf" ]] && filter="$filter,$vf"
  ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "$filter" \
    -f lavfi -i "sine=frequency=440:sample_rate=48000" \
    -t "$seconds" -map 0:v -map 1:a \
    -c:v libx264 -preset veryfast -pix_fmt yuv420p -g $((fps * 2)) -b:v 6M -maxrate 8M -bufsize 8M \
    -c:a "$acodec" -ac 2 -b:a 192k \
    -f mpegts "$out"
  echo "wrote $out"
}

make sample1080.ts  1920x1080 50 "" aac
make motion1080p.ts 1920x1080 25 "scroll=horizontal=0.02:vertical=0.01" aac
make sport720p60.ts 1280x720  60 "scroll=horizontal=0.03:vertical=0.015" ac3
