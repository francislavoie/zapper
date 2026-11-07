#!/usr/bin/env bash
# Generates the synthetic test streams tools/fake_stream.py serves: ffmpeg's
# test pattern and a tone, encoded the way live IPTV feeds usually are
# (H.264 8-bit 4:2:0 in MPEG-TS, a keyframe every two seconds), and one the
# way much broadcast SD still is (MPEG-2 video, MP2 sound).
#
#   tools/make_fixtures.sh              # the fixtures below
#   tools/make_fixtures.sh --force      # regenerate ones that already exist
#
# They go to $ZAPPER_FIXTURES, by default ~/.cache/zapper/fixtures, outside
# the tree: they are build output, and *.ts is ignored here anyway.
#
#   sample1080.ts   h264 1920x1080@50, aac   the pattern with little motion
#   motion1080p.ts  h264 1920x1080@25, aac   the whole frame scrolling
#   sport720p60.ts  h264 1280x720@60,  ac3   60 fps, high motion, AC-3 audio
#   mpeg2sd.ts      mpeg2 720x576@25,  mp2   video no browser plays
#   tsjump.ts       h264 1280x720@25,  aac   two seconds, then two minutes
#                                            stamped ten minutes later
#
# All 30 s but tsjump.ts, a stream that opens with its timestamps ten minutes
# apart: its first GOP is stamped from zero and the rest from 600 s, as a
# server's leftover from earlier, sent ahead of live, is. Read as stream time
# it is ten minutes of data in after two seconds, which no amount of bytes
# received explains. `tsjump.ts.seconds` beside it says how long it really
# runs, for fake_stream.py's pacing (ffprobe measures first timestamp to last).
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

# make <name> <size> <fps> <extra video filter or ""> <audio codec> [video codec]
make() {
  local out="$dir/$1" size=$2 fps=$3 vf=$4 acodec=$5 vcodec=${6:-libx264}
  local vopts=(-c:v libx264 -preset veryfast)
  [[ $vcodec == libx264 ]] || vopts=(-c:v "$vcodec")
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
    "${vopts[@]}" -pix_fmt yuv420p -g $((fps * 2)) -b:v 6M -maxrate 8M -bufsize 8M \
    -c:a "$acodec" -ac 2 -b:a 192k \
    -f mpegts "$out"
  echo "wrote $out"
}

# tsjump: two encodes, the second stamped 600 s on, one after the other.
make_jump() {
  local out="$dir/tsjump.ts" part
  if [[ -e "$out" && $force -eq 0 ]]; then
    echo "exists: $out (--force regenerates)"
    return
  fi
  part=$(mktemp -d)
  for spec in "head 2 0" "rest 120 600"; do
    read -r name secs offset <<<"$spec"
    ffmpeg -hide_banner -loglevel error -y \
      -f lavfi -i "testsrc2=size=1280x720:rate=25,scroll=horizontal=0.02:vertical=0.01" \
      -f lavfi -i "sine=frequency=440:sample_rate=48000" \
      -t "$secs" -map 0:v -map 1:a \
      -c:v libx264 -preset veryfast -pix_fmt yuv420p -g 50 -b:v 4M -maxrate 5M -bufsize 5M \
      -c:a aac -ac 2 -b:a 128k -output_ts_offset "$offset" \
      -f mpegts "$part/$name.ts"
  done
  cat "$part/head.ts" "$part/rest.ts" > "$out"
  rm -r "$part"
  echo 122 > "$out.seconds"
  echo "wrote $out"
}

make sample1080.ts  1920x1080 50 "" aac
make motion1080p.ts 1920x1080 25 "scroll=horizontal=0.02:vertical=0.01" aac
make sport720p60.ts 1280x720  60 "scroll=horizontal=0.03:vertical=0.015" ac3
make mpeg2sd.ts     720x576   25 "scroll=horizontal=0.02:vertical=0.01" mp2 mpeg2video
make_jump
