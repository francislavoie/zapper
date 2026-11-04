#!/usr/bin/env bash
# Usage: scripts/build-ffmpeg-static.sh <out-dir> [sources-dir]
#
# The FFmpeg the static zapper-server for Linux links in (docs/recording.md, "Without the app";
# docs/licensing.md, "The static zapper-server"): static libraries of libavformat, libavcodec,
# libswresample and libavutil, LGPL, built from the FFmpeg tarball packaging/libmpv/sources.txt
# pins (the same file, checked against the same SHA-256, as zapper's libmpv), with only what the
# recording service uses:
#
#   - MPEG-TS in (what it records) and out (the live feed it serves a browser), Matroska out (a
#     finished recording), from and to files and its own I/O callbacks: no network protocols;
#   - the parsers a stream's parameters are found with, and the bitstream filters the muxers
#     insert on their own;
#   - the decoders of the audio a browser cannot play (AC-3, E-AC-3, MP1/2/3, AAC and LATM, for
#     finding parameters), FFmpeg's own AAC encoder, and libswresample, to make AAC of it.
#
# Nothing else: no programs, no documentation, no hardware acceleration, no libraries found on
# the build machine (--disable-autodetect), nothing GPL or "nonfree". Run on a musl system (the
# Alpine image scripts/build-server-static.sh uses), so that the libraries link into a musl
# binary.
#
# Writes <out-dir>/include and <out-dir>/lib (the four .a), and the notice the binary carries
# (<out-dir>/FFMPEG-NOTICE.txt: what was built, how, from where, how to relink, then FFmpeg's
# COPYING.LGPLv2.1). Does nothing when <out-dir> was made by this same script from the same
# source (its .key). The tarball is downloaded into [sources-dir] (default: <out-dir>/../sources)
# unless it is there already.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
die() { echo "build-ffmpeg-static: $*" >&2; exit 1; }
[ $# -ge 1 ] || die "usage: $0 <out-dir> [sources-dir]"
out=$1
sources=${2:-$(dirname "$out")/sources}
line=$(awk '$1 == "ffmpeg" { print; exit }' "$root/packaging/libmpv/sources.txt")
[ -n "$line" ] || die "no ffmpeg in packaging/libmpv/sources.txt"
read -r _ version _ sum url <<<"$line"
file=ffmpeg-$version.tar.xz

# What the build is made from: the source and this script (its flags).
key=$( { echo "$line"; cat "$0"; } | sha256sum | cut -d' ' -f1)
if [ -f "$out/.key" ] && [ "$(cat "$out/.key")" = "$key" ] && [ -f "$out/lib/libavformat.a" ]; then
  echo "build-ffmpeg-static: $out is up to date (FFmpeg $version)"
  exit 0
fi

mkdir -p "$sources"
if [ ! -f "$sources/$file" ] || [ "$(sha256sum "$sources/$file" | cut -d' ' -f1)" != "$sum" ]; then
  echo "build-ffmpeg-static: downloading $url"
  curl -fsSL --retry 3 -o "$sources/$file.part" "$url"
  mv "$sources/$file.part" "$sources/$file"
fi
got=$(sha256sum "$sources/$file" | cut -d' ' -f1)
[ "$got" = "$sum" ] || die "$file: SHA-256 is $got, sources.txt says $sum"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tar -xJf "$sources/$file" -C "$work"
src=$work/ffmpeg-$version
[ -x "$src/configure" ] || die "no configure in $file"

flags=(
  --prefix="$out"
  --enable-static --disable-shared --enable-pic
  --disable-programs --disable-doc --disable-debug
  --disable-autodetect --disable-network --disable-hwaccels
  --disable-everything
  --disable-avdevice --disable-avfilter --disable-swscale
  --enable-avformat --enable-avcodec --enable-swresample
  --enable-protocol=file,pipe
  --enable-demuxer=mpegts,mov,matroska,avi
  --enable-muxer=mpegts,matroska
  --enable-parser=h264,hevc,mpegvideo,mpeg4video,vc1,av1,mpegaudio,aac,aac_latm,ac3,dca,opus,flac,vorbis,dvbsub,dvd_nav
  --enable-bsf=aac_adtstoasc,extract_extradata,h264_mp4toannexb,hevc_mp4toannexb,vvc_mp4toannexb,av1_frame_split
  --enable-decoder=aac,aac_latm,ac3,eac3,mp1,mp1float,mp2,mp2float,mp3,mp3float,dca,opus,flac,vorbis,subrip,ass,ssa,movtext,webvtt
  --enable-encoder=aac
)
log=$work/configure.log
(cd "$src" && ./configure "${flags[@]}") > "$log" 2>&1 || { tail -40 "$log"; die "configure failed"; }
# LGPL, and nothing that would make it otherwise.
grep -q '^License: LGPL version 2.1 or later' "$log" || { cat "$log"; die "this FFmpeg is not LGPL 2.1 or later"; }
make -C "$src" -j"${JOBS:-$(nproc)}" >/dev/null
rm -rf "$out"
make -C "$src" install >/dev/null
rm -rf "$out/share" "$out/lib/pkgconfig"

{
  cat <<EOF
FFmpeg $version, built into zapper-server
==========================================

This zapper-server is a single file with FFmpeg's libavformat, libavcodec, libswresample and
libavutil linked into it (statically), version $version, built by zapper's
scripts/build-ffmpeg-static.sh with nothing under the GPL and nothing "nonfree": they are under
the GNU Lesser General Public License, version 2.1 or later, whose text follows.

  FFmpeg   https://ffmpeg.org

The source
----------

The exact source, $file (SHA-256 $sum), from

  $url

is also published on zapper's GitHub releases page, in the libmpv release named in
packaging/libmpv/manifest.json (zapper's libmpv is built from the same tarball):

  https://github.com/francislavoie/zapper/releases

It was configured with:

  ./configure ${flags[*]/--prefix=*/}

Relinking it with another FFmpeg
--------------------------------

The LGPL lets you use a modified FFmpeg in its place. zapper's own source, all of it, with the
scripts that build this binary, is at https://github.com/francislavoie/zapper under the MIT
license: build your FFmpeg as static libraries (scripts/build-ffmpeg-static.sh shows how, with
these options), name its folder in ZAPPER_FFMPEG_STATIC, and build zapper-server for
x86_64-unknown-linux-musl as scripts/build-server-static.sh does. docs/licensing.md describes it.

EOF
  cat "$src/COPYING.LGPLv2.1"
} > "$out/FFMPEG-NOTICE.txt"
echo "$key" > "$out/.key"
echo "build-ffmpeg-static: FFmpeg $version in $out"
ls -l "$out/lib"
