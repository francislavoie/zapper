#!/usr/bin/env bash
# Generates the synthetic test streams tools/fake_stream.py serves: ffmpeg's
# test pattern and a tone, encoded the way live IPTV feeds usually are
# (H.264 8-bit 4:2:0 in MPEG-TS, a keyframe every two seconds), and one the
# way much broadcast SD still is (MPEG-2 video, MP2 sound).
#
#   tools/make_fixtures.sh              # the fixtures below (the films 120 s, the rest 30 s)
#   tools/make_fixtures.sh --force      # regenerate ones that already exist
#
# They go to $ZAPPER_FIXTURES, by default ~/.cache/zapper/fixtures, outside
# the tree: they are build output, and *.ts is ignored here anyway.
#
#   sample1080.ts   h264 1920x1080@50, aac   the pattern with little motion
#   motion1080p.ts  h264 1920x1080@25, aac   the whole frame scrolling
#   sport720p60.ts  h264 1280x720@60,  ac3   60 fps, high motion, AC-3 audio
#   mpeg2sd.ts      mpeg2 720x576@25,  mp2   video no browser plays
#   movie.mp4       h264 640x360@25,   aac   a 120 s "film": faststart, a keyframe every 2 s,
#                                            one mov_text subtitle track (English)
#   movie.mkv       h264 640x360@25,   ac3   the same in Matroska, with a subrip (English) and an
#                                            ass (French) subtitle track; all the text is made up
#   placeholder.ts  h264 1920x1080@30, aac   600 s of black and silence at a low bitrate: what
#                                            some providers send instead of a channel that is
#                                            off the air (fake_stream.py --placeholder)
#   radio.ts        no video,          aac   a radio channel: sound alone
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

make sample1080.ts  1920x1080 50 "" aac
make motion1080p.ts 1920x1080 25 "scroll=horizontal=0.02:vertical=0.01" aac
make sport720p60.ts 1280x720  60 "scroll=horizontal=0.03:vertical=0.015" ac3
make mpeg2sd.ts     720x576   25 "scroll=horizontal=0.02:vertical=0.01" mp2 mpeg2video

# movie <name> <audio codec> <subtitle codec>...
# A "film" for the on-demand tests: 120 s of the test pattern and a tone, with
# made-up timed subtitle lines (one every 10 s) embedded as text tracks.
movie() {
  local out="$dir/$1" acodec=$2; shift 2
  if [[ -e "$out" && $force -eq 0 ]]; then
    echo "exists: $out (--force regenerates)"
    return
  fi
  # A folder of its own, so the file can be named .srt: ffmpeg reads the format
  # from the extension, and BSD mktemp (macOS) has no --suffix.
  local tmp; tmp=$(mktemp -d)
  local srt="$tmp/subtitles.srt"
  local i
  for ((i = 0; i < 12; i++)); do
    printf '%d\n00:%02d:%02d,000 --> 00:%02d:%02d,000\nMade-up line %d of the test film.\n\n' \
      $((i + 1)) $(((i * 10 + 1) / 60)) $(((i * 10 + 1) % 60)) $(((i * 10 + 5) / 60)) $(((i * 10 + 5) % 60)) $((i + 1))
  done >"$srt"
  # The first track is English, the second French (made-up text in both).
  local langs=(eng fra)
  local inputs=() maps=(-map 0:v -map 1:a) opts=() n=0 codec
  for codec in "$@"; do
    inputs+=(-i "$srt")
    maps+=(-map "$((n + 2)):s")
    opts+=("-c:s:$n" "$codec" "-metadata:s:s:$n" language=${langs[$n]})
    n=$((n + 1))
  done
  ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=640x360:rate=25" \
    -f lavfi -i "sine=frequency=330:sample_rate=48000" \
    "${inputs[@]}" -t 120 "${maps[@]}" \
    -c:v libx264 -preset veryfast -pix_fmt yuv420p -g 50 -keyint_min 50 -sc_threshold 0 -b:v 800k \
    -c:a "$acodec" -ac 2 -b:a 96k "${opts[@]}" \
    -movflags +faststart "$out"
  rm -rf "$tmp"
  echo "wrote $out"
}

movie movie.mp4 aac mov_text
movie movie.mkv ac3 subrip ass

# The placeholder clip a provider answers an off-air channel's live URL with:
# ten minutes of black and silence, 1080p at 30 fps and a couple of hundred
# kbit/s, timestamps continuous throughout. fake_stream.py --placeholder sends
# it all at once and closes the connection, as such a provider does.
placeholder() {
  local out="$dir/placeholder.ts"
  if [[ -e "$out" && $force -eq 0 ]]; then
    echo "exists: $out (--force regenerates)"
    return
  fi
  ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "color=c=black:size=1920x1080:rate=30" \
    -f lavfi -i "anullsrc=channel_layout=stereo:sample_rate=48000" \
    -t 600 -map 0:v -map 1:a \
    -c:v libx264 -preset veryfast -tune stillimage -pix_fmt yuv420p -g 60 -b:v 160k -maxrate 200k -bufsize 400k \
    -c:a aac -ac 2 -b:a 32k \
    -f mpegts "$out"
  echo "wrote $out"
}

placeholder

# A radio channel: AAC in MPEG-TS with no video track at all.
radio="$dir/radio.ts"
if [[ -e "$radio" && $force -eq 0 ]]; then
  echo "exists: $radio (--force regenerates)"
else
  ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "sine=frequency=330:sample_rate=48000" \
    -t "$seconds" -map 0:a -c:a aac -ac 2 -b:a 128k \
    -f mpegts "$radio"
  echo "wrote $radio"
fi
